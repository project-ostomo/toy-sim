use super::*;
use osg_model::diplomacy::DeclarationCategory;
use osg_model::society::SocietyPresentation;
use osg_ui::components::badge;

pub fn bloc_color(
    directory: &SocietyPresentation,
    bloc: &osg_model::diplomacy::PoliticalBloc,
) -> egui::Color32 {
    let affiliation = bloc
        .members
        .iter()
        .find_map(|id| directory.sovereignties.get(id));
    match affiliation.map(|polity| polity.bloc) {
        Some(ownership::Bloc::Union) => INFO,
        Some(ownership::Bloc::League) => MINT,
        _ => ACCENT,
    }
}

fn identity_icon(
    directory: &SocietyPresentation,
    principal: Principal,
    size: f32,
) -> egui::RichText {
    let (icon, color) = match principal {
        Principal::Player(_) => (Icon::User, ACCENT),
        Principal::Organization(_) => (Icon::Shield, MUTED),
        Principal::Sovereignty(id) => {
            let color = directory
                .diplomacy
                .blocs
                .values()
                .find(|bloc| bloc.members.contains(&id))
                .map_or(WARNING, |bloc| bloc_color(directory, bloc));
            (Icon::Flag, color)
        }
    };
    icon.text(size).color(color)
}

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.);
    ui.label(egui::RichText::new(title).size(11.).color(MUTED));
    ui.add_space(4.);
}

pub fn row(ui: &mut egui::Ui, index: usize, body: impl FnOnce(&mut egui::Ui)) {
    let fill = if index % 2 == 0 {
        egui::Color32::from_rgb(21, 31, 41)
    } else {
        egui::Color32::TRANSPARENT
    };
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            body(ui);
        });
}

pub fn tab(ui: &mut egui::Ui, selected: bool, label: &str) -> bool {
    let response = ui.add(
        egui::Button::new(egui::RichText::new(label).color(if selected { ACCENT } else { MUTED }))
            .frame(false),
    );
    if selected {
        ui.painter().line_segment(
            [response.rect.left_bottom(), response.rect.right_bottom()],
            egui::Stroke::new(2., ACCENT),
        );
    }
    response.clicked()
}

pub fn draw(ui: &mut egui::Ui, state: &mut State, model: &FrameModel, intents: &mut Vec<Intent>) {
    let snapshot = model.society;
    if state.tab == Tab::Politics && state.politics.is_blocs() && state.selected_bloc.is_none() {
        state.selected_bloc = snapshot.directory.diplomacy.blocs.keys().next().copied();
    }
    let width = ui.available_width();
    let sidebar_width = if width >= 840. {
        260.
    } else {
        (width * 0.28).clamp(165., 230.)
    };
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(sidebar_width, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(sidebar_width);
                egui::ScrollArea::vertical()
                    .id_salt("directory_sidebar")
                    .auto_shrink([false, false])
                    .show(ui, |ui| sidebar(ui, state, snapshot, intents));
            },
        );
        ui.separator();
        let detail_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(detail_width, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(detail_width);
                egui::ScrollArea::vertical()
                    .id_salt("directory_detail")
                    .show(ui, |ui| {
                        ui.set_max_width(detail_width);
                        detail(ui, state, model, intents);
                    });
            },
        );
    });
}

pub fn choose(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &SocietyData,
    principal: Principal,
    prefix: &str,
) {
    let directory = &snapshot.directory;
    let selected = state.selected == Some(principal) && state.selected_bloc.is_none();
    let shortcut = matches!(prefix, "You" | "Organization" | "Polity");
    let war = !shortcut
        && directory.political_posture(Principal::Player(snapshot.account), principal)
            == Some(Standing::Hostile);
    let response = identity_row(
        ui,
        identity_icon(directory, principal, 14.),
        shortcut.then_some(prefix),
        &name(directory, principal),
        selected,
        war,
    );
    if response.clicked() {
        state.inspect(principal);
    }
}

fn sidebar(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &SocietyData,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    let me = Principal::Player(snapshot.account);
    for principal in directory.lineage(me) {
        let prefix = match principal {
            Principal::Player(_) => "You",
            Principal::Organization(_) => "Organization",
            Principal::Sovereignty(_) => "Polity",
        };
        choose(ui, state, snapshot, principal, prefix);
    }
    for bloc in directory.diplomacy.blocs.values().filter(|bloc| {
        directory
            .lineage(me)
            .iter()
            .any(|p| matches!(p, Principal::Sovereignty(id) if bloc.members.contains(id)))
    }) {
        if bloc_entry(ui, bloc, ACCENT, state.selected_bloc == Some(bloc.id), true) {
            state.selected_bloc = Some(bloc.id);
            state.tab = Tab::Politics;
            state.politics.select("blocs");
        }
    }
    ui.separator();
    if ui
        .add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text("Search directory…")
                .desired_width(ui.available_width()),
        )
        .changed()
    {
        state.search = state.search.chars().take(128).collect();
    }
    tree::draw(ui, state, snapshot);
    ui.separator();
    ui.collapsing("Administration", |ui| {
        for (tab, label) in [
            (Tab::Assets, "Asset permissions"),
            (Tab::ComputerGas, "Computer gas"),
        ] {
            if ui.button(label).clicked() {
                state.tab = tab;
                state.selected_bloc = None;
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut state.organization_name)
                .hint_text("New organization name…")
                .desired_width(ui.available_width()),
        );
        if ui
            .add_enabled(
                !state.organization_name.trim().is_empty() && state.organization_name.len() <= 128,
                egui::Button::new("Create organization"),
            )
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
}

pub fn bloc_entry(
    ui: &mut egui::Ui,
    bloc: &osg_model::diplomacy::PoliticalBloc,
    color: egui::Color32,
    selected: bool,
    shortcut: bool,
) -> bool {
    identity_row(
        ui,
        Icon::Handshake.text(14.).color(color),
        shortcut.then_some("Bloc"),
        &bloc.name,
        selected,
        false,
    )
    .clicked()
}

fn identity_row(
    ui: &mut egui::Ui,
    icon: egui::RichText,
    prefix: Option<&str>,
    name: &str,
    selected: bool,
    war: bool,
) -> egui::Response {
    let label_width = ui
        .painter()
        .layout_no_wrap(
            "Organization".into(),
            egui::FontId::proportional(10.),
            MUTED,
        )
        .size()
        .x;

    egui::Frame::new()
        .fill(if selected {
            egui::Color32::from_rgb(37, 69, 84)
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(5, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(20., 18.),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(20.);
                        ui.label(icon);
                    },
                );
                if let Some(prefix) = prefix {
                    ui.allocate_ui_with_layout(
                        egui::vec2(label_width, 18.),
                        egui::Layout::left_to_right(egui::Align::Min),
                        |ui| {
                            ui.set_min_width(label_width);
                            ui.label(egui::RichText::new(prefix).size(10.).color(MUTED));
                        },
                    );
                }
                let width = (ui.available_width() - if war { 42. } else { 0. }).max(0.);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 18.),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(width);
                        let label = egui::Label::new(egui::RichText::new(name).size(13.));
                        ui.add(if prefix.is_some() {
                            label.wrap()
                        } else {
                            label.truncate()
                        });
                    },
                );
                if war {
                    badge(ui, "WAR", THREAT);
                }
            });
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_text(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_opens_with_authenticated_identity_and_keeps_navigation_after_request_failure() {
        let ctx = egui::Context::default();
        osg_ui::theme::install(&ctx);
        let account = Id([42; 16]);
        let mut state = State::new(account);
        let mut snapshot = SocietyData::default();
        let navigation = NavigationCatalogue::default();
        let render = |state: &mut State, snapshot: &SocietyData| {
            let model = FrameModel {
                services: empty_services(),
                declaration_history: &[],
                declaration_history_next: None,
                declaration_history_key: None,
                industry: empty_industry(),

                navigation_status: &NavigationStatus::Ready,
                navigation_hash: None,
                navigation: &navigation,
                inhabited: Default::default(),
                society: snapshot,
                rows: Vec::new(),
                ship: None,
                details: None,
                system: String::new(),
                vicinity: String::new(),
                connected: true,
                status: "",
                time_ns: 0,
                calendar_unix_ms: 0,
                diagnostics: Default::default(),
                orbits: false,
            };
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 700.),
                    )),
                    ..Default::default()
                },
                |ui| super::super::draw(ui, state, &model, &mut Vec::new()),
            )
        };

        snapshot.account = account;
        snapshot.directory.viewer = account;
        snapshot
            .directory
            .administered
            .insert(Principal::Player(account));
        snapshot
            .directory
            .ancestry
            .insert(Principal::Player(account), vec![Principal::Player(account)]);
        snapshot.directory.players.insert(
            account,
            ownership::PlayerAffiliation {
                account,
                name: "Directory recovery pilot".into(),
                organization: None,
            },
        );
        let mut first = render(&mut state, &snapshot);
        assert_eq!(state.selected, Some(Principal::Player(account)));
        first.textures_delta.clear();
        state.error = Some("Request unavailable".into());
        let mut output = render(&mut state, &snapshot);
        assert_eq!(state.selected, Some(Principal::Player(account)));
        assert_eq!(state.politics.owner, state.selected);
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Directory recovery pilot"))
        }), "the loaded directory must remain visible alongside the request error");
        output.textures_delta.clear();
    }

    #[test]
    fn shortcut_values_share_a_column_and_long_values_stay_inside_it() {
        for width in [185., 260.] {
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            let rows = [
                ("You", "Pilot"),
                ("Organization", "Helion Flight Cooperative with a long name"),
                ("Polity", "Helion Commonwealth"),
                ("Bloc", "League of Free States"),
            ];
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(width);
                for (prefix, name) in rows {
                    identity_row(ui, Icon::User.text(14.), Some(prefix), name, false, false);
                }
            });
            output.textures_delta.clear();
            let positions: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape {
                        if rows.iter().any(|(_, name)| *name == text.galley.text()) {
                            return Some((text.pos, text.galley.size()));
                        }
                    }
                    None
                })
                .collect();
            assert_eq!(positions.len(), rows.len());
            for (position, size) in &positions {
                assert!((position.x - positions[0].0.x).abs() < 0.1);
                assert!(position.x + size.x <= width + 0.5);
            }
            assert!(positions[1].1.y > positions[0].1.y);
        }
    }
}

fn detail(ui: &mut egui::Ui, state: &mut State, model: &FrameModel, intents: &mut Vec<Intent>) {
    let snapshot = model.society;
    let directory = &snapshot.directory;
    if let Some(bloc) = state
        .selected_bloc
        .and_then(|id| directory.diplomacy.blocs.get(&id))
    {
        ui.horizontal(|ui| {
            ui.label(Icon::Handshake.text(24.).color(bloc_color(directory, bloc)));
            ui.heading(&bloc.name);
        });
        ui.weak(format!("Bloc · {} member polities", bloc.members.len()));
        ui.horizontal_wrapped(|ui| {
            badge(ui, "BLOC", INFO);
            if bloc.officers.contains(&snapshot.account) {
                badge(ui, "YOU ARE A BLOC OFFICER", ACCENT);
            }
        });
        state.politics.bloc = Some(bloc.id);
        state.politics.owner = directory
            .lineage(Principal::Player(snapshot.account))
            .into_iter()
            .find(|principal| matches!(principal, Principal::Sovereignty(_)));
        politics::draw(ui, &mut state.politics, snapshot, model, intents);
        return;
    }
    let Some(principal) = state.selected else {
        ui.weak("Loading identity…");
        return;
    };
    if !directory.contains(principal) {
        ui.weak("Identity unavailable. Select another entry from the directory.");
        return;
    }
    state.politics.owner = Some(principal);
    ui.horizontal(|ui| {
        ui.label(identity_icon(directory, principal, 24.));
        ui.vertical(|ui| {
            ui.heading(name(directory, principal));
            let subtitle = match principal {
                Principal::Sovereignty(id) => format!(
                    "Polity · {}",
                    if state
                        .tree
                        .complete(crate::state::requests::directory::Branch::Organizations(id))
                    {
                        format!(
                            "{} organizations",
                            directory
                                .organizations
                                .values()
                                .filter(|org| org.sovereignty == id)
                                .count()
                        )
                    } else {
                        "Loading organizations…".into()
                    }
                ),
                Principal::Organization(id) => directory.organizations.get(&id).map_or_else(
                    || "Organization".into(),
                    |org| {
                        format!(
                            "Organization · {}",
                            name(directory, Principal::Sovereignty(org.sovereignty))
                        )
                    },
                ),
                Principal::Player(_) => format!("Player · {}", lineage(directory, principal)),
            };
            ui.label(egui::RichText::new(subtitle).size(12.).color(MUTED));
        });
    });
    ui.horizontal_wrapped(|ui| {
        if directory
            .lineage(Principal::Player(snapshot.account))
            .contains(&principal)
        {
            badge(
                ui,
                match principal {
                    Principal::Player(_) => "YOU",
                    Principal::Organization(_) => "YOUR ORGANIZATION",
                    Principal::Sovereignty(_) => "YOUR POLITY",
                },
                ACCENT,
            );
        }
        if directory.administers(snapshot.account, principal)
            && !matches!(principal, Principal::Player(_))
        {
            badge(ui, "OFFICER", ACCENT);
        }
    });
    ui.add_space(6.);
    ui.horizontal_wrapped(|ui| {
        if tab(
            ui,
            state.tab == Tab::Directory && !state.members,
            "Overview",
        ) {
            state.tab = Tab::Directory;
            state.members = false;
        }
        for (key, label) in [
            ("declarations", "Declarations"),
            ("agreements", "Agreements"),
        ] {
            if tab(
                ui,
                state.tab == Tab::Politics && state.politics.is_tab(key),
                label,
            ) {
                state.tab = Tab::Politics;
                state.politics.select(key);
            }
        }
        if matches!(principal, Principal::Organization(_))
            && tab(ui, state.members && state.tab == Tab::Directory, "Members")
        {
            state.tab = Tab::Directory;
            state.members = true;
        }
        if matches!(principal, Principal::Player(_))
            && tab(
                ui,
                state.tab == Tab::Politics && state.politics.is_tab("sources"),
                "Trust sources",
            )
        {
            state.tab = Tab::Politics;
            state.politics.select("sources");
        }
    });
    ui.add_space(8.);
    match state.tab {
        Tab::Politics => politics::draw(ui, &mut state.politics, snapshot, model, intents),
        Tab::Assets => assets_panel(ui, state, snapshot, intents),
        Tab::ComputerGas => gas_accounts(ui, snapshot),
        Tab::Directory if state.members => {
            if let Principal::Organization(id) = principal {
                state.tree.branch_status(
                    ui,
                    crate::state::requests::directory::Branch::Players(Some(id)),
                );
                if state
                    .tree
                    .complete(crate::state::requests::directory::Branch::Players(Some(id)))
                {
                    members(ui, snapshot, principal, intents);
                }
            }
        }
        Tab::Directory => {
            if let Principal::Organization(id) = principal {
                if let Some(profile) = organizations::profile(id.0) {
                    profile_record(ui, state, profile);
                    ui.separator();
                }
            }
            overview(ui, snapshot, principal, intents);
        }
    }
}

fn overview(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    principal: Principal,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    if let Principal::Sovereignty(id) = principal {
        let bloc = directory
            .diplomacy
            .blocs
            .values()
            .find(|bloc| bloc.members.contains(&id));
        if let Some(bloc) = bloc {
            let pending = bloc.withdrawals.contains(&id);
            egui::Frame::new()
                .fill(SURFACE_RAISED)
                .stroke(egui::Stroke::new(
                    1.,
                    if pending { WARNING } else { BORDER },
                ))
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(Icon::Handshake.text(14.).color(bloc_color(directory, bloc)));
                        ui.label(format!("Member of {}", bloc.name));
                        badge(ui, "MEMBER", INFO);
                    });
                    if pending {
                        ui.colored_label(
                            WARNING,
                            "Withdrawal requested · waiting for a bloc officer to grant or refuse.",
                        );
                        ui.weak("Until then this polity carries the bloc's war and peace posture.");
                    } else {
                        ui.weak("War and peace posture is carried from this bloc.");
                    }
                    if directory.administers(snapshot.account, principal)
                        && ui
                            .small_button(if pending {
                                "Cancel request"
                            } else {
                                "Request withdrawal"
                            })
                            .clicked()
                    {
                        intents.push(Intent::Society(
                            SocietyCommand::Diplomacy(
                                osg_model::diplomacy::DiplomacyCommand::RequestBlocWithdrawal {
                                    bloc: bloc.id,
                                    polity: id,
                                    request: !pending,
                                },
                            ),
                            "Request bloc withdrawal",
                        ));
                    }
                });
        }
        section(ui, "WAR / PEACE POSTURE");
        let targets: Vec<_> = directory
            .sovereignties
            .values()
            .filter(|other| other.id != id)
            .collect();
        for (index, target) in targets.iter().enumerate() {
            row(ui, index, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(identity_icon(
                        directory,
                        Principal::Sovereignty(target.id),
                        14.,
                    ));
                    ui.label(&target.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let posture = directory
                            .political_posture(principal, Principal::Sovereignty(target.id));
                        let war = posture == Some(Standing::Hostile);
                        badge(
                            ui,
                            if war { "WAR" } else { "PEACE" },
                            if war { THREAT } else { MUTED },
                        );
                    });
                });
            });
        }
        section(ui, "JURISDICTION & RECOGNITION");
        let mut count = 0;
        for declaration in directory.diplomacy.declarations.values().filter(|d| {
            d.source == principal
                && d.enabled
                && matches!(
                    d.category,
                    DeclarationCategory::Claim | DeclarationCategory::Recognition
                )
        }) {
            row(ui, count, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(name(directory, declaration.target));
                    badge(
                        ui,
                        &format!("{:?}", declaration.category).to_uppercase(),
                        INFO,
                    );
                });
                if !declaration.note.is_empty() {
                    ui.weak(&declaration.note);
                }
            });
            count += 1;
        }
        if count == 0 {
            ui.weak("No public claims or recognition declarations.");
        }
    } else {
        if let Some(report) = snapshot
            .standing_report
            .as_ref()
            .filter(|report| report.target == principal)
        {
            standing_card(ui, directory, report);
        }
        personal_standing(ui, snapshot, principal, intents);
        membership(ui, snapshot, principal, intents);
    }
}

pub fn personal_standing(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    principal: Principal,
    intents: &mut Vec<Intent>,
) {
    let observer = Principal::Player(snapshot.account);
    let current = snapshot
        .directory
        .standings
        .get(&(observer, principal))
        .copied();
    ui.horizontal_wrapped(|ui| {
        ui.weak("Your standing");
        for (value, label, color) in [
            (None, "Inherit", MUTED),
            (Some(Standing::Friendly), "Friendly", POSITIVE),
            (Some(Standing::Neutral), "Neutral", MUTED),
            (Some(Standing::Hostile), "Hostile", THREAT),
        ] {
            if ui
                .selectable_label(current == value, egui::RichText::new(label).color(color))
                .clicked()
            {
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
}

fn members(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    principal: Principal,
    intents: &mut Vec<Intent>,
) {
    let Principal::Organization(id) = principal else {
        return;
    };
    let Some(org) = snapshot.directory.organizations.get(&id) else {
        return;
    };
    let members: Vec<_> = snapshot
        .directory
        .players
        .values()
        .filter(|p| p.organization == Some(id))
        .collect();
    personal_standing(ui, snapshot, principal, intents);
    section(ui, &format!("MEMBERS · {}", members.len()));
    if members
        .iter()
        .any(|member| member.account == snapshot.account)
        && ui
            .small_button(egui::RichText::new("Leave organization").color(THREAT))
            .clicked()
    {
        intents.push(Intent::Society(
            SocietyCommand::SetMembership {
                account: snapshot.account,
                organization: None,
            },
            "Leave organization",
        ));
    }
    for (index, member) in members.iter().enumerate() {
        row(ui, index, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(Icon::User.text(14.).color(POSITIVE));
                if ui
                    .add(egui::Button::new(&member.name).frame(false))
                    .clicked()
                {
                    intents.push(Intent::InspectAffiliation(Principal::Player(
                        member.account,
                    )));
                }
                if org.officers.contains(&member.account) {
                    badge(ui, "OFFICER", ACCENT);
                }
                if member.account == snapshot.account {
                    ui.weak("You");
                }
                if org.officers.contains(&snapshot.account) && member.account != snapshot.account {
                    let officer = org.officers.contains(&member.account);
                    if ui
                        .small_button(if officer {
                            "Revoke officer"
                        } else {
                            "Appoint officer"
                        })
                        .clicked()
                    {
                        intents.push(Intent::Society(
                            SocietyCommand::SetOfficer {
                                organization: id,
                                account: member.account,
                                officer: !officer,
                            },
                            "Update officer role",
                        ));
                    }
                    if ui
                        .small_button(egui::RichText::new("Remove").color(THREAT))
                        .clicked()
                    {
                        intents.push(Intent::Society(
                            SocietyCommand::SetMembership {
                                account: member.account,
                                organization: None,
                            },
                            "Remove member",
                        ));
                    }
                }
            });
        });
    }
    ui.add_space(8.);
    ui.weak("Players belong to one organization at a time. Organization officers manage membership and officer roles.");
}
