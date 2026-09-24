use super::*;
use osg_ui::components;
use ownership::{AccessProfile, Principal, SocietyCommand};
use std::collections::BTreeSet;

mod browser;

#[derive(Default, Resource)]
pub(super) struct State {
    browser: browser::State,
    search: String,
    selected: BTreeSet<Id>,
    owner: Option<Principal>,
    profile: Option<Id>,
    profile_name: String,
    profile_draft: Option<AccessProfile>,
    profile_grant: Option<Principal>,
}

impl State {
    pub fn query(&mut self, open: bool) -> Option<osg_model::assets::AssetsQuery> {
        self.browser.query(open)
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    snapshot: Option<&osg_model::assets::AssetsSnapshot>,
    intents: &mut Vec<Intent>,
) {
    ui.painter()
        .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, SURFACE);
    state.selected.retain(|id| {
        model
            .society
            .assets
            .iter()
            .any(|asset| asset.entity == *id && asset.can_manage)
    });
    components::action_header(
        ui,
        "assets_header",
        "Assets",
        "Ships, installations and goods across authorized inventories",
        |ui| {
            if let Some(snapshot) = snapshot {
                ui.label(format!(
                    "{} assets · {} kinds of goods",
                    snapshot.total_assets, snapshot.total_goods
                ));
            }
        },
    );
    browser::draw(ui, state, model, snapshot, intents);
    if !state.selected.is_empty() {
        ui.add_space(8.);
        egui::Frame::new()
            .fill(ACCENT.gamma_multiply(0.18))
            .inner_margin(8.)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                bulk_actions(ui, state, model, intents);
            });
    }
}

fn bulk_actions(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let directory = &model.society.directory;
    ui.label(format!(
        "{} selected · changes may succeed or fail independently",
        state.selected.len()
    ));
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("asset_profile")
            .selected_text(
                state
                    .profile
                    .and_then(|id| directory.access_profiles.get(&id))
                    .map_or("Permission profile…", |profile| &profile.name),
            )
            .show_ui(ui, |ui| {
                for profile in directory
                    .access_profiles
                    .values()
                    .filter(|profile| directory.administers(model.society.account, profile.owner))
                {
                    ui.selectable_value(&mut state.profile, Some(profile.id), &profile.name);
                }
            });
        if ui
            .add_enabled(
                model.connected && state.profile.is_some(),
                egui::Button::new("Link profile"),
            )
            .clicked()
        {
            for &asset in &state.selected {
                intents.push(Intent::Society(
                    SocietyCommand::ApplyAccessProfile {
                        asset,
                        profile: state.profile.unwrap(),
                    },
                    "Link permission profile",
                ));
            }
        }
        if ui
            .add_enabled(
                model.connected,
                egui::Button::new("Unlink · keep permissions"),
            )
            .clicked()
        {
            for &asset in &state.selected {
                intents.push(Intent::Society(
                    SocietyCommand::UnlinkAccessProfile { asset },
                    "Unlink permission profile",
                ));
            }
        }
        if ui
            .add_enabled(state.profile.is_some(), egui::Button::new("Edit profile"))
            .clicked()
        {
            state.profile_draft = state
                .profile
                .and_then(|id| directory.access_profiles.get(&id))
                .cloned();
        }
        if ui
            .add_enabled(
                model.connected && state.profile.is_some(),
                egui::Button::new("Delete profile"),
            )
            .clicked()
        {
            intents.push(Intent::Society(
                SocietyCommand::DeleteAccessProfile {
                    id: state.profile.unwrap(),
                },
                "Delete profile",
            ));
            state.profile = None;
        }
        egui::ComboBox::from_id_salt("asset_transfer_owner")
            .selected_text(state.owner.map_or_else(
                || "New owner…".into(),
                |owner| society::name(directory, owner),
            ))
            .show_ui(ui, |ui| {
                for owner in directory
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
                {
                    ui.selectable_value(
                        &mut state.owner,
                        Some(owner),
                        society::name(directory, owner),
                    );
                }
            });
        if ui
            .add_enabled(
                model.connected && state.owner.is_some(),
                egui::Button::new("Transfer ownership"),
            )
            .clicked()
        {
            for &asset in &state.selected {
                intents.push(Intent::Society(
                    SocietyCommand::TransferAsset {
                        asset,
                        owner: state.owner.unwrap(),
                    },
                    "Transfer ownership",
                ));
            }
        }
    });
    if let Some(draft) = &mut state.profile_draft {
        ui.label("Edit linked profile · saving updates all attached assets");
        ui.text_edit_singleline(&mut draft.name);
        egui::ScrollArea::vertical()
            .id_salt("profile_editor")
            .max_height(230.)
            .show(ui, |ui| {
                society::policy_editor(ui, directory, &mut draft.policy, &mut state.profile_grant);
            });
        let mut close = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    model.connected && draft.policy.valid() && !draft.name.trim().is_empty(),
                    egui::Button::new("Save profile"),
                )
                .clicked()
            {
                intents.push(Intent::Society(
                    SocietyCommand::SaveAccessProfile(draft.clone()),
                    "Update linked profile",
                ));
                close = true;
            }
            if ui.button("Close editor").clicked() {
                close = true;
            }
        });
        if close {
            state.profile_draft = None;
        }
    }
    if state.selected.len() == 1 {
        let asset = model
            .society
            .assets
            .iter()
            .find(|asset| state.selected.contains(&asset.entity))
            .unwrap();
        if let Some(binding) = directory.access_bindings.get(&asset.entity) {
            ui.label("Asset overrides · block permissions inherited from the profile");
            let mut denied = binding.denied.clone();
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                for permission in [
                    ownership::Permission::Navigate,
                    ownership::Permission::Dock,
                    ownership::Permission::View,
                    ownership::Permission::Control,
                    ownership::Permission::Configure,
                    ownership::Permission::TransferCargo,
                    ownership::Permission::Industry,
                    ownership::Permission::ManageAccess,
                ] {
                    let mut blocked = denied.contains(&permission);
                    if ui
                        .checkbox(&mut blocked, format!("{permission:?}"))
                        .changed()
                    {
                        changed = true;
                        if blocked {
                            denied.insert(permission);
                        } else {
                            denied.remove(&permission);
                        }
                    }
                }
            });
            if changed {
                intents.push(Intent::Society(
                    SocietyCommand::SetAccessDenied {
                        asset: asset.entity,
                        denied,
                    },
                    "Override profile permissions",
                ));
            }
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.profile_name)
                    .hint_text("New permission profile name…")
                    .char_limit(128),
            );
            if ui
                .add_enabled(
                    model.connected && !state.profile_name.trim().is_empty(),
                    egui::Button::new("Save selected asset’s permissions"),
                )
                .clicked()
            {
                let asset = model
                    .society
                    .assets
                    .iter()
                    .find(|asset| state.selected.contains(&asset.entity))
                    .unwrap();
                intents.push(Intent::Society(
                    SocietyCommand::SaveAccessProfile(AccessProfile {
                        id: Id::new(),
                        owner: Principal::Player(model.society.account),
                        name: state.profile_name.clone(),
                        policy: asset.access.clone(),
                    }),
                    "Save permission profile",
                ));
            }
        });
    }
}
