use super::*;
use osg_model::diplomacy::*;

#[derive(Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Declarations,
    Agreements,
    Blocs,
    Sources,
}

pub(super) struct State {
    pub(super) bloc: Option<Id>,
    bloc_tab: usize,
    pub history: Option<(Principal, DeclarationCategory, Principal)>,
    pub history_before: Option<u64>,
    history_next: Option<u64>,
    history_page: Vec<Declaration>,
    tab: Tab,
    pub(super) owner: Option<Principal>,
    target: Option<Principal>,
    category: DeclarationCategory,
    standing: Standing,
    note: String,
    title: String,
    terms: String,
    structured_terms: Vec<AgreementTerm>,
    tariff_basis_points: u16,
    bloc_name: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            bloc: None,
            bloc_tab: 0,
            tab: Tab::default(),
            history: None,
            history_before: None,
            history_next: None,
            history_page: Vec::new(),
            owner: None,
            target: None,
            category: DeclarationCategory::Standing,
            standing: Standing::Neutral,
            note: String::new(),
            title: String::new(),
            terms: String::new(),
            structured_terms: Vec::new(),
            tariff_basis_points: 200,
            bloc_name: String::new(),
        }
    }
}

impl State {
    pub(super) fn select(&mut self, key: &str) {
        self.tab = match key {
            "agreements" => Tab::Agreements,
            "blocs" => Tab::Blocs,
            "sources" => Tab::Sources,
            _ => Tab::Declarations,
        };
    }

    pub(super) fn is_blocs(&self) -> bool {
        self.tab == Tab::Blocs
    }

    pub(super) fn is_tab(&self, key: &str) -> bool {
        matches!(
            (&self.tab, key),
            (Tab::Declarations, "declarations")
                | (Tab::Agreements, "agreements")
                | (Tab::Sources, "sources")
        )
    }
}

#[cfg(test)]
impl State {
    pub(super) fn gallery_tab(&mut self, variant: &str) {
        self.tab = match variant {
            "agreements" => Tab::Agreements,
            "blocs" => Tab::Blocs,
            "sources" => Tab::Sources,
            _ => Tab::Declarations,
        };
        if variant == "history" {
            self.history = Some((
                Principal::Sovereignty(Id([3; 16])),
                DeclarationCategory::Standing,
                Principal::Sovereignty(Id([4; 16])),
            ));
        }
        self.owner = Some(if variant == "sources" {
            Principal::Player(Id([1; 16]))
        } else {
            Principal::Sovereignty(Id([3; 16]))
        });
        self.target = Some(Principal::Sovereignty(Id([4; 16])));
    }
}

fn emit(intents: &mut Vec<Intent>, command: DiplomacyCommand) {
    intents.push(Intent::Society(
        SocietyCommand::Diplomacy(command),
        "Diplomacy",
    ));
}

fn category(ui: &mut egui::Ui, value: &mut DeclarationCategory) {
    egui::ComboBox::from_id_salt("declaration_category")
        .selected_text(format!("{value:?}"))
        .show_ui(ui, |ui| {
            for item in [
                DeclarationCategory::Standing,
                DeclarationCategory::Wanted,
                DeclarationCategory::Embargo,
                DeclarationCategory::Licence,
                DeclarationCategory::Claim,
                DeclarationCategory::Recognition,
            ] {
                ui.selectable_value(value, item, format!("{item:?}"));
            }
        });
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &SocietyData,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    if model.declaration_history_key == state.history {
        state.history_page = model.declaration_history.to_vec();
        state.history_next = model.declaration_history_next;
    }
    let directory = &snapshot.directory;
    let owner = state.owner.unwrap_or(Principal::Player(snapshot.account));
    if matches!(state.tab, Tab::Sources) {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("diplomacy_target")
                .selected_text(
                    state
                        .target
                        .map_or_else(|| "Choose a principal…".into(), |p| name(directory, p)),
                )
                .show_ui(ui, |ui| {
                    for principal in principals(directory).filter(|p| *p != owner) {
                        ui.selectable_value(
                            &mut state.target,
                            Some(principal),
                            name(directory, principal),
                        );
                    }
                });
            if matches!(state.tab, Tab::Declarations | Tab::Sources) {
                category(ui, &mut state.category);
            }
        });
    }
    match state.tab {
        Tab::Declarations => {
            declarations(
                ui,
                state,
                owner,
                directory,
                directory.administers(snapshot.account, owner),
                intents,
            );
            ui.separator();
            agreements(ui, state, owner, snapshot, intents);
            presentation::section(ui, "HOW OTHERS LIST THIS PRINCIPAL");
            for (index, declaration) in directory
                .diplomacy
                .declarations
                .values()
                .filter(|d| d.target == owner && d.enabled)
                .enumerate()
            {
                presentation::row(ui, index, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name(directory, declaration.source));
                        osg_ui::components::badge(
                            ui,
                            &format!("{:?}", declaration.category).to_uppercase(),
                            INFO,
                        );
                    });
                    ui.weak(&declaration.note);
                });
            }
        }
        Tab::Agreements => agreements(ui, state, owner, snapshot, intents),
        Tab::Sources => sources(ui, state, owner, directory, intents),
        Tab::Blocs => blocs(ui, state, owner, snapshot, intents),
    }
}

fn target_picker(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    directory: &OwnershipDirectory,
) {
    egui::ComboBox::from_id_salt("diplomacy_edit_target")
        .selected_text(
            state
                .target
                .map_or_else(|| "Choose a principal…".into(), |p| name(directory, p)),
        )
        .show_ui(ui, |ui| {
            for principal in principals(directory).filter(|p| *p != owner) {
                ui.selectable_value(
                    &mut state.target,
                    Some(principal),
                    name(directory, principal),
                );
            }
        });
}

fn declarations(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    directory: &OwnershipDirectory,
    can_edit: bool,
    intents: &mut Vec<Intent>,
) {
    presentation::section(ui, "PUBLISHED DECLARATIONS · ALL PUBLIC");
    ui.add_enabled_ui(can_edit, |ui| {
        ui.collapsing("Publish declaration…", |ui| {
            target_picker(ui, state, owner, directory);
            category(ui, &mut state.category);
            ui.horizontal_wrapped(|ui| {
                for standing in [Standing::Friendly, Standing::Neutral, Standing::Hostile] {
                    ui.selectable_value(&mut state.standing, standing, format!("{standing:?}"));
                }
                ui.add(
                    egui::TextEdit::singleline(&mut state.note)
                        .hint_text("Public note…")
                        .char_limit(512),
                );
                if ui
                    .add_enabled(state.target.is_some(), egui::Button::new("Publish"))
                    .clicked()
                {
                    let target = state.target.unwrap();
                    let revision = directory
                        .diplomacy
                        .declarations
                        .get(&(owner, state.category, target))
                        .map_or(0, |d| d.revision);
                    emit(
                        intents,
                        DiplomacyCommand::Publish(Declaration {
                            source: owner,
                            target,
                            category: state.category,
                            revision,
                            standing: state.standing,
                            enabled: true,
                            note: state.note.clone(),
                        }),
                    );
                }
            });
        });
    });
    for (index, declaration) in directory
        .diplomacy
        .declarations
        .values()
        .filter(|d| d.source == owner)
        .enumerate()
    {
        ui.push_id((declaration.category, declaration.target), |ui| {
            presentation::row(ui, index, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let color = if matches!(
                        declaration.category,
                        DeclarationCategory::Wanted | DeclarationCategory::Embargo
                    ) {
                        THREAT
                    } else {
                        MUTED
                    };
                    osg_ui::components::badge(
                        ui,
                        &format!("{:?}", declaration.category).to_uppercase(),
                        color,
                    );
                    ui.label(name(directory, declaration.target))
                        .on_hover_text(&declaration.note);
                    if ui
                        .small_button(format!("v{} · history", declaration.revision))
                        .clicked()
                    {
                        state.history =
                            Some((declaration.source, declaration.category, declaration.target));
                        state.history_before = None;
                        state.history_page.clear();
                        state.history_next = None;
                    }
                    ui.menu_button("⋯", |ui| {
                        ui.label(&declaration.note);
                        if can_edit
                            && declaration.enabled
                            && ui.small_button("Withdraw declaration").clicked()
                        {
                            let mut withdrawn = declaration.clone();
                            withdrawn.enabled = false;
                            emit(intents, DiplomacyCommand::Publish(withdrawn));
                        }
                    });
                });
            });
        });
    }
    if let Some((source, category, target)) = state.history {
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!(
                "{category:?} · {} · history",
                name(directory, target)
            ));
            if ui.small_button("Close").clicked() {
                state.history = None;
            }
            if ui
                .add_enabled(state.history_before.is_some(), egui::Button::new("Latest"))
                .clicked()
            {
                state.history_before = None;
            }
            if ui
                .add_enabled(state.history_next.is_some(), egui::Button::new("Older"))
                .clicked()
            {
                state.history_before = state.history_next;
            }
        });
        if state.history == Some((source, category, target)) {
            for entry in &state.history_page {
                ui.label(format!(
                    "Revision {} · {:?} · {}",
                    entry.revision,
                    entry.standing,
                    if entry.enabled {
                        "published"
                    } else {
                        "withdrawn"
                    }
                ));
                ui.weak(&entry.note);
            }
        }
    }
}

fn sources(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    directory: &OwnershipDirectory,
    intents: &mut Vec<Intent>,
) {
    presentation::section(ui, "WAR / PEACE · FIXED BY AFFILIATION");
    presentation::row(ui, 0, |ui| {
        ui.label(lineage(directory, owner));
        ui.weak("Political posture follows affiliation. Personal standings are resolved below.");
    });
    presentation::section(ui, "STANDINGS & LISTS · FIRST MATCH WINS");
    let categories = [
        (DeclarationCategory::Standing, "Standing"),
        (DeclarationCategory::Wanted, "Wanted"),
        (DeclarationCategory::Embargo, "Embargo"),
        (DeclarationCategory::Licence, "Licensed"),
    ];
    let mut sources = directory
        .diplomacy
        .trust
        .get(&(owner, state.category))
        .cloned()
        .unwrap_or_default();
    for &(category, _) in &categories {
        for &source in directory
            .diplomacy
            .trust
            .get(&(owner, category))
            .into_iter()
            .flatten()
        {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
    }
    egui::ScrollArea::horizontal()
        .id_salt("trust_matrix_scroll")
        .show(ui, |ui| {
            egui::Grid::new("trust_matrix")
                .striped(true)
                .spacing([12., 10.])
                .show(ui, |ui| {
                    ui.weak("#");
                    ui.weak("Source");
                    for &(_, label) in &categories {
                        ui.label(egui::RichText::new(label).size(11.).color(MUTED));
                    }
                    ui.end_row();
                    ui.weak("—");
                    ui.label("Personal declarations");
                    for &(category, _) in &categories {
                        let mut present = directory
                            .diplomacy
                            .declarations
                            .values()
                            .any(|d| d.source == owner && d.category == category && d.enabled);
                        ui.add_enabled(false, egui::Checkbox::without_text(&mut present));
                    }
                    ui.end_row();
                    for (index, &source) in sources.iter().enumerate() {
                        ui.weak(format!("{}", index + 1));
                        ui.label(name(directory, source));
                        for &(category, _) in &categories {
                            let mut order = directory
                                .diplomacy
                                .trust
                                .get(&(owner, category))
                                .cloned()
                                .unwrap_or_default();
                            let mut enabled = order.contains(&source);
                            if ui.checkbox(&mut enabled, "").changed() {
                                if enabled {
                                    order.push(source);
                                } else {
                                    order.retain(|p| *p != source);
                                }
                                emit(
                                    intents,
                                    DiplomacyCommand::SetTrust {
                                        owner,
                                        category,
                                        sources: order,
                                    },
                                );
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    ui.add_space(8.);
    ui.collapsing("Manage sources & priority", |ui| {
        sources_editor(ui, state, owner, directory, intents)
    });
    presentation::section(ui, "PREVIEW");
    for (index, principal) in principals(directory)
        .filter(|p| *p != owner)
        .take(3)
        .enumerate()
    {
        let (standing, source) = directory.standing_with_source(owner, principal);
        presentation::row(ui, index, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(name(directory, principal));
                ui.colored_label(
                    super::super::super::standing::color(Some(standing)),
                    format!("{standing:?}"),
                );
            });
            if let ownership::StandingSource::Declaration { source, .. } = source {
                ui.weak(format!("from {}", name(directory, source)));
            }
        });
    }
}

fn sources_editor(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    directory: &OwnershipDirectory,
    intents: &mut Vec<Intent>,
) {
    ui.weak(format!("{:?} priority · reorder sources", state.category));
    let sources = directory
        .diplomacy
        .trust
        .get(&(owner, state.category))
        .cloned()
        .unwrap_or_default();
    if ui
        .add_enabled(
            state.target.is_some_and(|p| !sources.contains(&p)) && sources.len() < 32,
            egui::Button::new("Add selected source"),
        )
        .clicked()
    {
        let mut changed = sources.clone();
        changed.push(state.target.unwrap());
        emit(
            intents,
            DiplomacyCommand::SetTrust {
                owner,
                category: state.category,
                sources: changed,
            },
        );
    }
    for (index, source) in sources.iter().enumerate() {
        ui.push_id(source, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{}. {}", index + 1, name(directory, *source)));
                let mut changed = sources.clone();
                let mut update = false;
                if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked() {
                    changed.swap(index, index - 1);
                    update = true;
                }
                if ui
                    .add_enabled(index + 1 < sources.len(), egui::Button::new("↓"))
                    .clicked()
                {
                    changed.swap(index, index + 1);
                    update = true;
                }
                if ui.button("Remove").clicked() {
                    changed.remove(index);
                    update = true;
                }
                if update {
                    emit(
                        intents,
                        DiplomacyCommand::SetTrust {
                            owner,
                            category: state.category,
                            sources: changed,
                        },
                    );
                }
            });
        });
    }
    if let Some(target) = state.target {
        if let Some(declaration) = directory.diplomacy.resolve(owner, state.category, target) {
            ui.separator();
            ui.label(format!(
                "Resolved from {} · revision {} · {:?}",
                name(directory, declaration.source),
                declaration.revision,
                declaration.standing
            ));
        }
    }
}

fn agreements(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    snapshot: &SocietyData,
    intents: &mut Vec<Intent>,
) {
    presentation::section(ui, "AGREEMENTS");
    ui.collapsing("Propose agreement…", |ui| {
        target_picker(ui, state, owner, &snapshot.directory);
        ui.add(
            egui::TextEdit::singleline(&mut state.title)
                .hint_text("Agreement title")
                .char_limit(128),
        );
        ui.add(
            egui::TextEdit::multiline(&mut state.terms)
                .hint_text("Agreement note")
                .char_limit(2048)
                .desired_rows(3),
        );
        ui.horizontal_wrapped(|ui| {
            for (term, label) in [
                (AgreementTerm::DockingAccess, "Docking"),
                (AgreementTerm::BasingAccess, "Basing"),
                (AgreementTerm::HonorWanted, "Honour wanted lists"),
                (AgreementTerm::MutualDefence, "Mutual defence"),
            ] {
                let mut enabled = state.structured_terms.contains(&term);
                if ui.checkbox(&mut enabled, label).changed() {
                    if enabled {
                        state.structured_terms.push(term);
                    } else {
                        state.structured_terms.retain(|item| item != &term);
                    }
                }
            }
        });
        let mut tariff = state
            .structured_terms
            .iter()
            .any(|term| matches!(term, AgreementTerm::Tariff { .. }));
        ui.horizontal(|ui| {
            if ui.checkbox(&mut tariff, "Tariff").changed() {
                state
                    .structured_terms
                    .retain(|term| !matches!(term, AgreementTerm::Tariff { .. }));
                if tariff {
                    state.structured_terms.push(AgreementTerm::Tariff {
                        basis_points: state.tariff_basis_points,
                    });
                }
            }
            if tariff {
                ui.add(
                    egui::DragValue::new(&mut state.tariff_basis_points)
                        .range(0..=10_000)
                        .suffix(" bp"),
                );
                for term in &mut state.structured_terms {
                    if let AgreementTerm::Tariff { basis_points } = term {
                        *basis_points = state.tariff_basis_points;
                    }
                }
            }
        });
        if ui
            .add_enabled(
                state.target.is_some()
                    && !state.title.trim().is_empty()
                    && !state.structured_terms.is_empty(),
                egui::Button::new("Propose agreement"),
            )
            .clicked()
        {
            emit(
                intents,
                DiplomacyCommand::ProposeAgreement {
                    from: owner,
                    to: state.target.unwrap(),
                    title: state.title.clone(),
                    terms: state.structured_terms.clone(),
                    note: state.terms.clone(),
                },
            );
        }
    });
    for (index, agreement) in snapshot
        .directory
        .diplomacy
        .agreements
        .values()
        .filter(|a| a.from == owner || a.to == owner)
        .enumerate()
    {
        ui.push_id(agreement.id, |ui| {
            presentation::row(ui, index, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&agreement.title);
                    osg_ui::components::badge(
                        ui,
                        &format!("{:?}", agreement.status).to_uppercase(),
                        match agreement.status {
                            AgreementStatus::Active => POSITIVE,
                            AgreementStatus::Suspended => WARNING,
                            _ => MUTED,
                        },
                    );
                });
                ui.label(format!(
                    "{} ↔ {}",
                    name(&snapshot.directory, agreement.from),
                    name(&snapshot.directory, agreement.to)
                ));
                ui.horizontal_wrapped(|ui| {
                    for term in &agreement.terms {
                        let label = match term {
                            AgreementTerm::DockingAccess => "Docking access".into(),
                            AgreementTerm::BasingAccess => "Basing access".into(),
                            AgreementTerm::HonorWanted => "Honour wanted lists".into(),
                            AgreementTerm::MutualDefence => "Mutual defence".into(),
                            AgreementTerm::Tariff { basis_points } => {
                                format!("Tariff · {:.2}%", f64::from(*basis_points) / 100.)
                            }
                        };
                        osg_ui::components::badge(ui, &label.to_uppercase(), MUTED);
                    }
                });
                if !agreement.note.is_empty() {
                    ui.weak(&agreement.note);
                }
                let party = snapshot
                    .directory
                    .administers(snapshot.account, agreement.from)
                    || snapshot
                        .directory
                        .administers(snapshot.account, agreement.to);
                if party {
                    ui.horizontal(|ui| {
                        for (status, label, enabled) in [
                            (
                                AgreementStatus::Active,
                                "Accept",
                                agreement.status == AgreementStatus::Proposed
                                    && snapshot
                                        .directory
                                        .administers(snapshot.account, agreement.to),
                            ),
                            (
                                AgreementStatus::Suspended,
                                "Suspend",
                                agreement.status == AgreementStatus::Active,
                            ),
                            (
                                AgreementStatus::Terminated,
                                "Terminate / refuse",
                                agreement.status != AgreementStatus::Terminated,
                            ),
                        ] {
                            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                emit(
                                    intents,
                                    DiplomacyCommand::ChangeAgreement {
                                        id: agreement.id,
                                        expected_revision: agreement.revision,
                                        status,
                                    },
                                );
                            }
                        }
                    });
                }
            });
        });
    }
}

fn blocs(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    snapshot: &SocietyData,
    intents: &mut Vec<Intent>,
) {
    for bloc in snapshot
        .directory
        .diplomacy
        .blocs
        .values()
        .filter(|bloc| state.bloc.is_none_or(|id| id == bloc.id))
    {
        let officer = bloc.officers.contains(&snapshot.account);
        if !officer {
            presentation::section(ui, "OFFICERS");
            presentation::row(ui, 0, |ui| {
                for &account in &bloc.officers {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name(&snapshot.directory, Principal::Player(account)));
                        osg_ui::components::badge(ui, "OFFICER", ACCENT);
                    });
                }
                if bloc.officers.is_empty() {
                    ui.weak("No registered officers.");
                }
                ui.weak("Membership and posture decisions are signed by these accounts.");
            });
        }
        ui.horizontal_wrapped(|ui| {
            if officer
                && presentation::tab(
                    ui,
                    state.bloc_tab == 0,
                    &format!(
                        "Requests · {}",
                        bloc.withdrawals.len() + bloc.applications.len()
                    ),
                )
            {
                state.bloc_tab = 0;
            }
            if presentation::tab(
                ui,
                state.bloc_tab == 1 || (!officer && state.bloc_tab == 0),
                "Posture",
            ) {
                state.bloc_tab = 1;
            }
            if presentation::tab(
                ui,
                state.bloc_tab == 2,
                &format!("Members · {}", bloc.members.len()),
            ) {
                state.bloc_tab = 2;
            }
        });
        if officer && state.bloc_tab == 0 {
            presentation::section(ui, "PENDING REQUESTS");
            if bloc.withdrawals.is_empty() && bloc.applications.is_empty() {
                ui.weak("No pending requests.");
            }
            for &polity in &bloc.withdrawals {
                egui::Frame::new().fill(SURFACE_RAISED).stroke(egui::Stroke::new(1., WARNING)).inner_margin(10).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name(&snapshot.directory, Principal::Sovereignty(polity)));
                        osg_ui::components::badge(ui, "WITHDRAWAL", WARNING);
                    });
                    ui.weak("Asks to leave the bloc. Its current posture will be retained until it publishes its own.");
                    ui.horizontal_wrapped(|ui| {
                        for (grant, label) in [(true, "Grant withdrawal"), (false, "Refuse")] {
                            if ui.button(label).clicked() { emit(intents, DiplomacyCommand::DecideBlocWithdrawal { bloc: bloc.id, polity, grant }); }
                        }
                    });
                });
                ui.add_space(6.);
            }
            for &polity in &bloc.applications {
                presentation::row(ui, 0, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name(&snapshot.directory, Principal::Sovereignty(polity)));
                        osg_ui::components::badge(ui, "APPLICATION", ACCENT);
                    });
                    ui.weak("If admitted, this polity carries the bloc's posture.");
                    ui.horizontal(|ui| {
                        for (admit, label) in [(true, "Admit"), (false, "Refuse")] {
                            if ui.button(label).clicked() {
                                emit(
                                    intents,
                                    DiplomacyCommand::DecideApplication {
                                        bloc: bloc.id,
                                        polity,
                                        admit,
                                    },
                                );
                            }
                        }
                    });
                });
            }
        }
        if state.bloc_tab == 1 || (!officer && state.bloc_tab == 0) {
            presentation::section(ui, "WAR / PEACE POSTURE · CARRIED BY ALL MEMBERS");
            for (index, (&target, &standing)) in bloc.posture.iter().enumerate() {
                presentation::row(ui, index, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name(&snapshot.directory, Principal::Sovereignty(target)));
                        let war = standing == Standing::Hostile;
                        osg_ui::components::badge(
                            ui,
                            if war { "WAR" } else { "PEACE" },
                            if war { THREAT } else { MUTED },
                        );
                    });
                });
            }
            if bloc.posture.is_empty() {
                ui.weak("No published posture overrides.");
            }
        }
        presentation::section(ui, "MEMBERS");
        for (index, &polity) in bloc.members.iter().enumerate() {
            presentation::row(ui, index, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(name(&snapshot.directory, Principal::Sovereignty(polity)));
                    let pending = bloc.withdrawals.contains(&polity);
                    osg_ui::components::badge(
                        ui,
                        if pending {
                            "WITHDRAWAL PENDING"
                        } else {
                            "MEMBER"
                        },
                        if pending { WARNING } else { MUTED },
                    );
                    if officer
                        && ui
                            .small_button(egui::RichText::new("Remove").color(THREAT))
                            .clicked()
                    {
                        emit(
                            intents,
                            DiplomacyCommand::RemoveBlocMember {
                                bloc: bloc.id,
                                polity,
                            },
                        );
                    }
                });
            });
        }
    }
    ui.add_space(8.);
    ui.collapsing("Membership & posture actions", |ui| {
        blocs_editor(ui, state, owner, snapshot, intents)
    });
}

fn blocs_editor(
    ui: &mut egui::Ui,
    state: &mut State,
    owner: Principal,
    snapshot: &SocietyData,
    intents: &mut Vec<Intent>,
) {
    let polity = if let Principal::Sovereignty(id) = owner {
        Some(id)
    } else {
        None
    };
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.bloc_name)
                .hint_text("New bloc name")
                .char_limit(128),
        );
        if ui
            .add_enabled(
                polity.is_some() && !state.bloc_name.trim().is_empty(),
                egui::Button::new("Found bloc"),
            )
            .clicked()
        {
            emit(
                intents,
                DiplomacyCommand::CreateBloc {
                    name: state.bloc_name.clone(),
                    founder: polity.unwrap(),
                },
            );
        }
    });
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("posture_target")
            .selected_text(state.target.map_or_else(
                || "Posture / officer target…".into(),
                |p| name(&snapshot.directory, p),
            ))
            .show_ui(ui, |ui| {
                for principal in principals(&snapshot.directory)
                    .filter(|p| !matches!(p, Principal::Organization(_)))
                {
                    ui.selectable_value(
                        &mut state.target,
                        Some(principal),
                        name(&snapshot.directory, principal),
                    );
                }
            });
        for standing in [Standing::Friendly, Standing::Neutral, Standing::Hostile] {
            ui.selectable_value(&mut state.standing, standing, format!("{standing:?}"));
        }
        if let (Some(polity), Some(Principal::Sovereignty(target))) = (polity, state.target) {
            if ui.button("Set polity posture").clicked() {
                emit(
                    intents,
                    DiplomacyCommand::SetPosture {
                        polity,
                        target,
                        standing: state.standing,
                    },
                );
            }
        }
    });
    for bloc in snapshot
        .directory
        .diplomacy
        .blocs
        .values()
        .filter(|bloc| state.bloc.is_none_or(|id| id == bloc.id))
    {
        ui.push_id(bloc.id, |ui| {
            ui.separator();
            ui.heading(&bloc.name);
            ui.weak(format!(
                "{} members · {} officers · {} applications",
                bloc.members.len(),
                bloc.officers.len(),
                bloc.applications.len()
            ));
            if let Some(polity) = polity {
                if bloc.members.contains(&polity) {
                    let request = !bloc.withdrawals.contains(&polity);
                    if ui
                        .button(if request {
                            "Request withdrawal"
                        } else {
                            "Cancel withdrawal request"
                        })
                        .clicked()
                    {
                        emit(
                            intents,
                            DiplomacyCommand::RequestBlocWithdrawal {
                                bloc: bloc.id,
                                polity,
                                request,
                            },
                        );
                    }
                } else {
                    let apply = !bloc.applications.contains(&polity);
                    if ui
                        .button(if apply { "Apply" } else { "Cancel application" })
                        .clicked()
                    {
                        emit(
                            intents,
                            DiplomacyCommand::ApplyToBloc {
                                bloc: bloc.id,
                                polity,
                                apply,
                            },
                        );
                    }
                }
            }
            for &member in &bloc.members {
                ui.horizontal(|ui| {
                    ui.label(name(&snapshot.directory, Principal::Sovereignty(member)));
                    if bloc.officers.contains(&snapshot.account)
                        && ui.button("Remove member").clicked()
                    {
                        emit(
                            intents,
                            DiplomacyCommand::RemoveBlocMember {
                                bloc: bloc.id,
                                polity: member,
                            },
                        );
                    }
                });
            }
            if bloc.officers.contains(&snapshot.account) {
                if let Some(Principal::Player(account)) = state.target {
                    let officer = !bloc.officers.contains(&account);
                    if ui
                        .button(if officer {
                            "Appoint selected officer"
                        } else {
                            "Remove selected officer"
                        })
                        .clicked()
                    {
                        emit(
                            intents,
                            DiplomacyCommand::SetBlocOfficer {
                                bloc: bloc.id,
                                account,
                                officer,
                            },
                        );
                    }
                }
                if let Some(Principal::Sovereignty(target)) = state.target {
                    if ui
                        .button("Set bloc posture toward selected polity")
                        .clicked()
                    {
                        emit(
                            intents,
                            DiplomacyCommand::SetBlocPosture {
                                bloc: bloc.id,
                                target,
                                standing: state.standing,
                            },
                        );
                    }
                }
                for &applicant in &bloc.applications {
                    ui.horizontal(|ui| {
                        ui.label(name(&snapshot.directory, Principal::Sovereignty(applicant)));
                        for (admit, label) in [(true, "Admit"), (false, "Refuse")] {
                            if ui.button(label).clicked() {
                                emit(
                                    intents,
                                    DiplomacyCommand::DecideApplication {
                                        bloc: bloc.id,
                                        polity: applicant,
                                        admit,
                                    },
                                );
                            }
                        }
                    });
                }
            }
            for (&target, standing) in &bloc.posture {
                ui.label(format!(
                    "{:?} toward {}",
                    standing,
                    name(&snapshot.directory, Principal::Sovereignty(target))
                ));
            }
        });
    }
}
