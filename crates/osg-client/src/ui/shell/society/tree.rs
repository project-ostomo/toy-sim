use super::*;
use crate::state::requests::directory::{Branch, Status};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Node {
    Bloc(Id),
    Principal(Principal),
    Unaligned,
    Unaffiliated,
}

#[derive(Default)]
pub struct Tree {
    open: BTreeMap<Node, bool>,
    search_open: BTreeMap<Node, bool>,
    search: String,
    initialized: bool,
    pub reveal_selected: bool,
    pub requested: BTreeSet<Branch>,
    pub status: BTreeMap<Branch, Status>,
    pub search_status: Status,
    pub matches: BTreeSet<Principal>,
    context: Option<SessionKey>,
}

impl Tree {
    pub fn sync(&mut self, session: &crate::state::SocietyState) {
        if self.context != session.context {
            *self = Self {
                context: session.context,
                ..Default::default()
            };
        }
        self.status = session.directory.statuses();
        self.search_status = session.directory.search_status.clone();
        self.matches = session.directory.matches.clone();
    }

    fn reveal(&mut self, directory: &OwnershipDirectory, principal: Principal) {
        for ancestor in directory.lineage(principal) {
            if ancestor != principal {
                self.open.insert(Node::Principal(ancestor), true);
            }
            if let Principal::Sovereignty(id) = ancestor {
                let bloc = directory
                    .diplomacy
                    .blocs
                    .values()
                    .find(|bloc| bloc.members.contains(&id));
                self.open.insert(
                    bloc.map_or(Node::Unaligned, |bloc| Node::Bloc(bloc.id)),
                    true,
                );
            }
        }
        if let Principal::Player(id) = principal {
            if directory
                .players
                .get(&id)
                .is_some_and(|player| player.organization.is_none())
            {
                self.open.insert(Node::Unaffiliated, true);
            }
        }
    }

    pub fn complete(&self, branch: Branch) -> bool {
        self.status.get(&branch).is_some_and(|status| status.loaded)
    }

    pub fn branch_status(&self, ui: &mut egui::Ui, branch: Branch) {
        match self.status.get(&branch) {
            Some(Status {
                error: Some(error), ..
            }) => {
                ui.colored_label(THREAT, error);
            }
            Some(Status { loaded: true, .. }) => {}
            _ => {
                ui.weak("Loading…");
            }
        }
    }
}

fn arrow(ui: &mut egui::Ui, tree: &mut Tree, node: Node, searching: bool) -> bool {
    let entries = if searching {
        &mut tree.search_open
    } else {
        &mut tree.open
    };
    let open = entries.entry(node).or_insert(searching);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16., 22.), egui::Sense::click());
    if response.clicked() {
        *open = !*open;
    }
    let c = rect.center();
    let points = if *open {
        vec![
            c + egui::vec2(-4., -2.),
            c + egui::vec2(4., -2.),
            c + egui::vec2(0., 3.),
        ]
    } else {
        vec![
            c + egui::vec2(-2., -4.),
            c + egui::vec2(-2., 4.),
            c + egui::vec2(3., 0.),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        MUTED,
        egui::Stroke::NONE,
    ));
    *open
}

fn visible(
    state: &State,
    directory: &OwnershipDirectory,
    principal: Principal,
    inherited: bool,
) -> bool {
    inherited
        || state.search.trim().is_empty()
        || state
            .tree
            .matches
            .iter()
            .any(|matched| directory.lineage(*matched).contains(&principal))
}

fn principal_branch(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &SocietyData,
    principal: Principal,
    inherited: bool,
) {
    let directory = &snapshot.directory;
    if !visible(state, directory, principal, inherited) {
        return;
    }
    let searching = !state.search.trim().is_empty();
    let open = ui
        .horizontal(|ui| {
            let open = arrow(ui, &mut state.tree, Node::Principal(principal), searching);
            presentation::choose(ui, state, snapshot, principal, "");
            open
        })
        .inner;
    if !open {
        return;
    }
    let branch = match principal {
        Principal::Sovereignty(id) => Branch::Organizations(id),
        Principal::Organization(id) => Branch::Players(Some(id)),
        Principal::Player(_) => return,
    };
    state.tree.requested.insert(branch);
    let inherited = inherited || state.tree.matches.contains(&principal);
    ui.indent(principal, |ui| {
        state.tree.branch_status(ui, branch);
        let mut children: Vec<_> = match principal {
            Principal::Sovereignty(id) => directory
                .organizations
                .values()
                .filter(|org| org.sovereignty == id)
                .map(|org| Principal::Organization(org.id))
                .collect(),
            Principal::Organization(id) => directory
                .players
                .values()
                .filter(|player| player.organization == Some(id))
                .map(|player| Principal::Player(player.account))
                .collect(),
            Principal::Player(_) => Vec::new(),
        };
        children.sort_by_key(|child| (name(directory, *child).to_lowercase(), *child));
        if children.is_empty() && state.tree.complete(branch) {
            ui.weak("No members");
        }
        for child in children {
            if matches!(child, Principal::Player(_)) {
                if visible(state, directory, child, inherited) {
                    presentation::choose(ui, state, snapshot, child, "");
                }
            } else {
                principal_branch(ui, state, snapshot, child, inherited);
            }
        }
    });
}

pub(super) fn draw(ui: &mut egui::Ui, state: &mut State, snapshot: &SocietyData) {
    let directory = &snapshot.directory;
    state.tree.requested.clear();
    if !state.tree.initialized {
        state
            .tree
            .reveal(directory, Principal::Player(snapshot.account));
        state.tree.initialized = state.tree.complete(Branch::Blocs)
            && state.tree.complete(Branch::Polities)
            && directory
                .lineage(Principal::Player(snapshot.account))
                .iter()
                .all(|principal| directory.contains(*principal));
    }
    if state.tree.reveal_selected {
        if let Some(selected) = state.selected.filter(|principal| {
            directory
                .lineage(*principal)
                .iter()
                .all(|ancestor| directory.contains(*ancestor))
        }) {
            state.tree.reveal(directory, selected);
            state.tree.reveal_selected = false;
        }
    }
    let filter = state.search.trim().to_lowercase();
    if filter != state.tree.search {
        state.tree.search_open.clear();
        state.tree.search = filter.clone();
    }
    let searching = !filter.is_empty();
    if searching {
        if let Some(error) = &state.tree.search_status.error {
            ui.colored_label(THREAT, error);
        } else if !state.tree.search_status.loaded {
            ui.weak("Searching…");
        }
    }
    state.tree.branch_status(ui, Branch::Polities);
    state.tree.branch_status(ui, Branch::Blocs);
    let mut blocs: Vec<_> = directory.diplomacy.blocs.values().collect();
    blocs.sort_by_key(|bloc| (&bloc.name, bloc.id));
    let mut grouped = BTreeSet::<Id>::new();
    for bloc in blocs {
        grouped.extend(bloc.members.iter().copied());
        let matching = searching && bloc.name.to_lowercase().contains(&filter);
        if searching
            && !matching
            && !bloc
                .members
                .iter()
                .any(|id| visible(state, directory, Principal::Sovereignty(*id), false))
        {
            continue;
        }
        let open = ui
            .horizontal(|ui| {
                let open = arrow(ui, &mut state.tree, Node::Bloc(bloc.id), searching);
                if presentation::bloc_entry(
                    ui,
                    bloc,
                    presentation::bloc_color(directory, bloc),
                    state.selected_bloc == Some(bloc.id),
                    false,
                ) {
                    state.selected_bloc = Some(bloc.id);
                    state.tab = Tab::Politics;
                    state.politics.select("blocs");
                }
                open
            })
            .inner;
        if open {
            ui.indent(("bloc", bloc.id), |ui| {
                for id in &bloc.members {
                    principal_branch(ui, state, snapshot, Principal::Sovereignty(*id), matching);
                }
            });
        }
    }
    let open = ui
        .horizontal(|ui| {
            let open = arrow(ui, &mut state.tree, Node::Unaligned, searching);
            ui.label("Unaligned");
            open
        })
        .inner;
    if open {
        ui.indent("unaligned", |ui| {
            for id in directory
                .sovereignties
                .keys()
                .filter(|id| !grouped.contains(*id))
            {
                principal_branch(ui, state, snapshot, Principal::Sovereignty(*id), false);
            }
        });
    }
    let open = ui
        .horizontal(|ui| {
            let open = arrow(ui, &mut state.tree, Node::Unaffiliated, searching);
            ui.label("Unaffiliated players");
            open
        })
        .inner;
    if open {
        state.tree.requested.insert(Branch::Players(None));
        ui.indent("unaffiliated", |ui| {
            state.tree.branch_status(ui, Branch::Players(None));
            for player in directory
                .players
                .values()
                .filter(|player| player.organization.is_none())
            {
                let principal = Principal::Player(player.account);
                if visible(state, directory, principal, false) {
                    presentation::choose(ui, state, snapshot, principal, "");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_arrows_and_names_are_independent_and_search_preserves_toggles() {
        let id = Id([1; 16]);
        let principal = Principal::Sovereignty(id);
        let mut snapshot = SocietyData::default();
        snapshot.directory.sovereignties.insert(
            id,
            ownership::Sovereignty {
                id,
                name: "Home polity".into(),
                bloc: ownership::Bloc::NonAligned,
                officers: BTreeSet::new(),
            },
        );
        let ctx = egui::Context::default();
        osg_ui::theme::install(&ctx);
        let mut state = State::default();
        let frame = |state: &mut State, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400., 300.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(260.);
                    principal_branch(ui, state, &snapshot, principal, false);
                },
            );
            output.textures_delta.clear();
            output
        };
        frame(&mut state, Vec::new());
        let output = frame(&mut state, Vec::new());
        let arrow_position = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Path(path) = &shape.shape {
                    if path.points.len() == 3 {
                        return Some(path.points[0].lerp(path.points[1], 0.5));
                    }
                }
                None
            })
            .unwrap();
        let name_position = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    if text.galley.text() == "Home polity" {
                        return Some(text.pos + text.galley.size() * 0.5);
                    }
                }
                None
            })
            .unwrap();
        let click = |state: &mut State, pos| {
            for pressed in [true, false] {
                frame(
                    state,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        };

        click(&mut state, arrow_position);
        assert_eq!(state.selected, None);
        assert_eq!(state.tree.open[&Node::Principal(principal)], true);
        click(&mut state, name_position);
        assert_eq!(state.selected, Some(principal));
        assert_eq!(state.tree.open[&Node::Principal(principal)], true);
        click(&mut state, arrow_position);
        assert_eq!(state.selected, Some(principal));
        assert_eq!(state.tree.open[&Node::Principal(principal)], false);

        state.search = "Home".into();
        state.tree.matches.insert(principal);
        frame(&mut state, Vec::new());
        assert_eq!(state.tree.search_open[&Node::Principal(principal)], true);
        state.search.clear();
        frame(&mut state, Vec::new());
        assert_eq!(state.tree.open[&Node::Principal(principal)], false);
    }
}
