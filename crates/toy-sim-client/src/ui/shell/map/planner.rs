use super::*;
use std::time::Duration;
use toy_sim_model::routing;

#[derive(Default)]
pub(in super::super) struct Preview {
    context: Option<(Id, u64, u64)>,
    request: Option<routing::Request>,
    status: Option<routing::Status>,
    pending: Option<Id>,
    next_poll: Duration,
    committing: Option<Id>,
}

impl Preview {
    pub fn begin(
        &mut self,
        ship: &ShipTelemetry,
        orders: Vec<travel::Order>,
        append: bool,
        preferences: travel::PlanningPreferences,
        outgoing: &mut Outgoing,
    ) {
        let mut queue = if append {
            ship.travel
                .orders
                .iter()
                .skip(ship.travel.order)
                .map(|stage| stage.action.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        queue.extend(orders);

        self.context = Some((ship.ship, ship.authority_revision, ship.travel.revision));
        self.pending = None;
        self.committing = None;
        self.request = None;
        if queue.len() > routing::MAX_ORDERS {
            self.status = Some(routing::Status::Failed {
                reason: format!(
                    "A route can contain at most {} orders.",
                    routing::MAX_ORDERS
                ),
            });
            return;
        }

        let request = routing::Request {
            id: u64::from_le_bytes(Id::new().0[..8].try_into().unwrap()).max(1),
            orders: queue,
            preferences,
        };
        self.pending = Some(outgoing.push(Action::RouteRequest {
            ship: ship.ship,
            authority_revision: ship.authority_revision,
            request: request.clone(),
        }));
        self.request = Some(request);
        self.status = None;
    }

    pub fn update(
        &mut self,
        ship: Option<&ShipTelemetry>,
        results: &[CommandResult],
        outgoing: &mut Outgoing,
        now: Duration,
    ) {
        let context = ship.map(|ship| (ship.ship, ship.authority_revision, ship.travel.revision));
        if self.context != context {
            *self = Self::default();
            return;
        }
        if let Some(result) = self
            .committing
            .and_then(|id| results.iter().find(|result| result.id == id))
        {
            self.committing = None;
            if let Some(error) = &result.error {
                self.status = Some(routing::Status::Failed {
                    reason: error.clone(),
                });
            } else {
                *self = Self::default();
            }
            return;
        }

        if let Some(result) = self
            .pending
            .and_then(|id| results.iter().find(|result| result.id == id))
        {
            self.pending = None;
            self.status = Some(match (&result.error, &result.reply, &self.request) {
                (Some(error), _, _) => routing::Status::Failed {
                    reason: error.clone(),
                },
                (_, Some(Reply::Route { id, status }), Some(request)) if *id == request.id => {
                    status.clone()
                }
                _ => routing::Status::Failed {
                    reason: "The routing service returned an invalid response. Please retry."
                        .into(),
                },
            });
            self.next_poll = now + Duration::from_millis(250);
        }
        if self.pending.is_none()
            && now >= self.next_poll
            && matches!(self.status, Some(routing::Status::Pending { .. }))
        {
            if let (Some(ship), Some(request)) = (ship, &self.request) {
                self.pending = Some(outgoing.push(Action::RoutePoll {
                    ship: ship.ship,
                    authority_revision: ship.authority_revision,
                    id: request.id,
                }));
            }
        }
    }

    pub fn sent_commit(&mut self, id: Id) {
        self.committing = Some(id);
    }

    pub fn retry(&mut self, ship: &ShipTelemetry, outgoing: &mut Outgoing) {
        if let Some(request) = self.request.clone() {
            self.begin(ship, request.orders, false, request.preferences, outgoing);
        }
    }

    pub fn plan(&self) -> Option<&routing::Plan> {
        match &self.status {
            Some(routing::Status::Ready { plan }) => Some(plan),
            _ => None,
        }
    }

    pub fn commit(&self, ship: &ShipTelemetry) -> Option<ShipCommand> {
        if self.committing.is_some() {
            return None;
        }

        let plan = self.plan()?;
        let (owner, authority, revision) = self.context?;
        (owner == ship.ship
            && authority == ship.authority_revision
            && revision == ship.travel.revision
            && plan.travel_revision == ship.travel.revision)
            .then_some(ShipCommand::UseRoute {
                id: self.request.as_ref()?.id,
                expected_revision: plan.travel_revision,
                engage: true,
            })
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    preview: &Preview,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    if preview.request.is_none() && preview.status.is_none() {
        ui.weak("Preview a destination to compare time and fuel before engaging.");
        return;
    }
    ui.separator();
    ui.strong("ROUTE PREVIEW");
    match &preview.status {
        None => {
            ui.spinner();
            ui.weak("Submitting route request…");
        }
        Some(routing::Status::Pending { progress }) => {
            let travel = travel::TravelState {
                status: travel::Status::Planning,
                planning: Some(*progress),
                ..Default::default()
            };
            instruments::planning_progress(ui, &travel);
        }
        Some(routing::Status::Unknown) | Some(routing::Status::Failed { .. }) => {
            let reason = match &preview.status {
                Some(routing::Status::Failed { reason }) => reason.as_str(),
                _ => "Route preview is unavailable. Request a new estimate.",
            };
            ui.colored_label(THREAT, reason);
            if ui
                .add_enabled(preview.request.is_some(), egui::Button::new("Retry route"))
                .clicked()
            {
                intents.push(Intent::RetryRoute);
            }
        }
        Some(routing::Status::Ready { plan }) => {
            ui.horizontal(|ui| {
                let can_commit = model.connected
                    && model
                        .ship
                        .is_some_and(|ship| preview.commit(ship).is_some());
                if ui
                    .add_enabled(can_commit, egui::Button::new("Engage route"))
                    .clicked()
                {
                    intents.push(Intent::CommitRoute);
                }
                let total_ticks = plan
                    .orders
                    .iter()
                    .try_fold(0_u64, |elapsed, stage| match stage.action {
                        travel::Order::WaitUntil(until) => {
                            Some(elapsed.max(until.saturating_sub(plan.planned_tick)))
                        }
                        _ => elapsed.checked_add(stage.estimated_duration_ticks?),
                    });
                ui.weak(total_ticks.map_or_else(
                    || format!("{} stages · duration partly unknown", plan.orders.len()),
                    |ticks| {
                        format!(
                            "{} stages · estimated duration {}m {:02}s",
                            plan.orders.len(),
                            ticks / 600,
                            ticks / 10 % 60
                        )
                    },
                ));
            });
            ui.small("The flight computer generates each local maneuver during flight; route times and fuel are estimates.");
            if plan.fuel_budget.exhausted() {
                ui.colored_label(
                    THREAT,
                    "FUEL EXHAUSTION · this route may consume all available propulsion fuel",
                );
            }

            instruments::fuel_estimate(ui, &plan.fuel_budget, model, false);
            let mut arrival = Some(plan.planned_tick);

            for (index, stage) in plan.orders.iter().enumerate() {
                arrival = match stage.action {
                    travel::Order::WaitUntil(until) => arrival.map(|tick| tick.max(until)),
                    _ => arrival
                        .zip(stage.estimated_duration_ticks)
                        .map(|(tick, duration)| tick.saturating_add(duration)),
                };

                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "{}  {}",
                        index + 1,
                        instruments::order_label(
                            &stage.action,
                            model.navigation,
                            &model.celestial_systems
                        )
                    ));
                    ui.monospace(instruments::eta_label(stage, arrival, plan.planned_tick));
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship() -> ShipTelemetry {
        crate::ui::tests::ship(Id([42; 16])).0
    }

    fn ready(ship: &ShipTelemetry) -> routing::Status {
        routing::Status::Ready {
            plan: routing::Plan {
                planned_tick: 100,
                travel_revision: ship.travel.revision,
                topology_revision: 1,
                orders: vec![travel::QueuedOrder::estimated(travel::Order::Undock, 20.)],
                fuel_budget: travel::FuelBudget {
                    complete: true,
                    resources: vec![travel::FuelRequirement {
                        resource: "water".into(),
                        required_kg: 20.,
                        available_kg: 10.,
                    }],
                },
            },
        }
    }

    fn result(preview: &Preview, status: routing::Status) -> CommandResult {
        CommandResult {
            id: preview.pending.unwrap(),
            effective_tick: 100,
            error: None,
            reply: Some(Reply::Route {
                id: preview.request.as_ref().unwrap().id,
                status,
            }),
        }
    }

    #[test]
    fn long_ready_preview_scrolls_without_squeezing_or_growing_desktop_map() {
        let mut ship = ship();
        let routing::Status::Ready { mut plan } = ready(&ship) else {
            unreachable!();
        };
        plan.orders = vec![travel::QueuedOrder::estimated(travel::Order::Undock, 20.); 24];
        ship.travel.orders = vec![travel::QueuedOrder::estimated(travel::Order::Undock, 20.)];
        ship.travel.fuel_budget = Some(plan.fuel_budget.clone());

        let mut state = super::super::State::default();
        let mut outgoing = Outgoing::default();
        state.route.begin(
            &ship,
            vec![travel::Order::Undock],
            false,
            Default::default(),
            &mut outgoing,
        );
        let response = result(&state.route, routing::Status::Ready { plan });
        state
            .route
            .update(Some(&ship), &[response], &mut outgoing, Duration::ZERO);

        let catalogue = NavigationCatalogue::default();
        let society = ownership::SocietySnapshot::default();
        let model = super::super::tests::model(&catalogue, &society, Some(&ship));
        let context = egui::Context::default();
        toy_sim_ui::theme::install(&context);
        context.all_styles_mut(|style| style.animation_time = 0.);
        let mut desktop = Desktop::default();
        desktop.open(MAP);
        let mut settled = None;
        let mut canvas = egui::Rect::NOTHING;
        let mut final_stage_visible = false;

        for frame in 0..90 {
            let events = if frame >= 12 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(canvas.center().x, canvas.bottom() + 80.)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        phase: egui::TouchPhase::Move,
                        delta: egui::vec2(0., -80.),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            } else {
                Vec::new()
            };

            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1600., 1000.),
                    )),
                    time: Some(frame as f64 / 60.),
                    events,
                    ..Default::default()
                },
                |ui| {
                    desktop.show(ui.ctx(), MAP, |ui| {
                        super::super::draw(ui, &mut state, &model, &mut Vec::new());
                    });
                },
            );
            output.textures_delta.clear();

            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == egui::Color32::from_rgb(9, 17, 26) => {
                        canvas = rect.rect.intersect(shape.clip_rect);
                    }
                    egui::Shape::Text(text) if text.galley.job.text.starts_with("24  ") => {
                        let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                        final_stage_visible |= shape.clip_rect.contains(bounds.center());
                    }
                    _ => {}
                }
            }

            if frame >= 8 {
                assert!(canvas.height() >= 220., "map squeezed to {canvas:?}");
                let size = desktop.rect(MAP).unwrap().size();
                let expected = *settled.get_or_insert(size);
                assert!(
                    (size - expected).length() < 1.,
                    "map grew from {expected:?} to {size:?}"
                );
            }
        }

        assert!(
            final_stage_visible,
            "scrolling never revealed the final route stage"
        );
    }

    #[test]
    fn route_preview_appends_remaining_queue_and_correlates_poll_and_commit() {
        let mut ship = ship();
        ship.travel.orders = vec![
            travel::Order::Undock.into(),
            travel::Order::Jump(Id([2; 16])).into(),
            travel::Order::Dock(Id([3; 16])).into(),
        ];
        ship.travel.order = 1;
        let destination = travel::Order::TravelTo(travel::Destination::Beacon(Id([4; 16])));
        let preference = travel::PlanningPreferences {
            fuel_fraction: 0.42,
            ..Default::default()
        };
        let mut outgoing = Outgoing::default();
        let mut preview = Preview::default();
        preview.begin(
            &ship,
            vec![destination.clone()],
            true,
            preference,
            &mut outgoing,
        );

        let (_, Action::RouteRequest { request, .. }) = &outgoing.pending()[0] else {
            panic!("preview must use the public routing API");
        };
        assert_eq!(request.preferences, preference);
        assert_eq!(
            request.orders,
            vec![
                travel::Order::Jump(Id([2; 16])),
                travel::Order::Dock(Id([3; 16])),
                destination,
            ]
        );
        assert!(preview.commit(&ship).is_none());

        let unrelated = CommandResult {
            id: Id([90; 16]),
            effective_tick: 100,
            error: None,
            reply: Some(Reply::JoinedGroup(Id([91; 16]))),
        };
        preview.update(Some(&ship), &[unrelated], &mut outgoing, Duration::ZERO);
        assert!(preview.pending.is_some());
        assert_eq!(outgoing.pending().len(), 1);

        let pending = result(
            &preview,
            routing::Status::Pending {
                progress: travel::PlanningProgress {
                    stage: travel::PlanningStage::SearchingRoutes,
                    completed: 3,
                    total: Some(10),
                },
            },
        );
        preview.update(Some(&ship), &[pending], &mut outgoing, Duration::ZERO);
        preview.update(Some(&ship), &[], &mut outgoing, Duration::from_millis(249));
        assert_eq!(outgoing.pending().len(), 1);
        preview.update(Some(&ship), &[], &mut outgoing, Duration::from_millis(250));
        assert!(matches!(outgoing.pending()[1].1, Action::RoutePoll { .. }));

        let response = result(&preview, ready(&ship));
        preview.update(
            Some(&ship),
            &[response],
            &mut outgoing,
            Duration::from_millis(300),
        );
        assert!(preview.plan().unwrap().fuel_budget.exhausted());
        assert!(matches!(
            preview.commit(&ship),
            Some(ShipCommand::UseRoute { engage: true, .. })
        ));
        assert_eq!(
            outgoing.pending().len(),
            2,
            "ready does not engage until the user commits"
        );

        let id = outgoing.ship(&ship, preview.commit(&ship).unwrap());
        preview.sent_commit(id);
        assert!(
            preview.commit(&ship).is_none(),
            "one outstanding commit at a time"
        );
        preview.update(
            Some(&ship),
            &[CommandResult {
                id,
                effective_tick: 103,
                reply: None,
                error: Some("Ship configuration changed; request another plan".into()),
            }],
            &mut outgoing,
            Duration::from_millis(400),
        );
        assert!(matches!(
            preview.status,
            Some(routing::Status::Failed { .. })
        ));
        let previous_id = preview.request.as_ref().unwrap().id;
        preview.retry(&ship, &mut outgoing);
        assert_ne!(preview.request.as_ref().unwrap().id, previous_id);
    }

    #[test]
    fn route_preview_rejects_stale_ship_authority_and_queue() {
        for change in 0..4 {
            let mut ship = ship();
            let mut outgoing = Outgoing::default();
            let mut preview = Preview::default();
            preview.begin(
                &ship,
                vec![travel::Order::Undock],
                false,
                Default::default(),
                &mut outgoing,
            );
            let old = result(&preview, ready(&ship));
            match change {
                0 => ship.ship = Id([80; 16]),
                1 => ship.authority_revision += 1,
                2 => ship.travel.revision += 1,
                _ => {}
            }
            let focused = (change != 3).then_some(&ship);
            preview.update(focused, &[old.clone()], &mut outgoing, Duration::ZERO);
            preview.update(focused, &[old], &mut outgoing, Duration::from_secs(1));
            assert!(preview.plan().is_none());
            assert!(preview.request.is_none());
        }
    }

    #[test]
    fn strategic_preview_survives_motion_time_and_map_catalogue_updates() {
        let mut ship = ship();
        let mut outgoing = Outgoing::default();
        let mut preview = Preview::default();
        preview.begin(
            &ship,
            vec![travel::Order::Undock],
            false,
            Default::default(),
            &mut outgoing,
        );
        let response = result(&preview, ready(&ship));
        preview.update(Some(&ship), &[response], &mut outgoing, Duration::ZERO);

        ship.pose = Some(Pose {
            position: GalacticPosition::from_meters(glam::DVec3::splat(1e8)),
            velocity: [1200., 50., 20.],
            ..Default::default()
        });
        preview.update(Some(&ship), &[], &mut outgoing, Duration::from_secs(3600));
        assert!(preview.commit(&ship).is_some());

        let catalogue = NavigationCatalogue {
            topology_revision: 99,
            systems: vec![NavigationSystem {
                id: Id([3; 16]),
                name: "Local system".into(),
                position: GalacticPosition::ZERO,
                sovereignty: None,
                population: 0,
            }],
            ..Default::default()
        };
        let society = ownership::SocietySnapshot::default();
        let mut model = super::super::tests::model(&catalogue, &society, Some(&ship));
        model.navigation_hash = Some([2; 32]);
        let mut state = super::super::State {
            catalogue_hash: Some([1; 32]),
            route: preview,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        toy_sim_ui::theme::install(&ctx);
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 900.),
                    )),
                    ..Default::default()
                },
                |ui| super::super::draw(ui, &mut state, &model, &mut Vec::new()),
            );
            output.textures_delta.clear();
        }
        assert!(state.route.commit(&ship).is_some());
        assert_eq!(state.route.plan().unwrap().topology_revision, 1);
    }

    #[test]
    fn ready_preview_displays_server_stages_eta_and_fuel_warning_before_engage() {
        let ship = ship();
        let mut outgoing = Outgoing::default();
        let mut preview = Preview::default();
        preview.begin(
            &ship,
            vec![travel::Order::Undock],
            false,
            Default::default(),
            &mut outgoing,
        );
        let response = result(&preview, ready(&ship));
        preview.update(Some(&ship), &[response], &mut outgoing, Duration::ZERO);

        let catalogue = NavigationCatalogue::default();
        let society = ownership::SocietySnapshot::default();
        let model = super::super::tests::model(&catalogue, &society, Some(&ship));
        let ctx = egui::Context::default();
        toy_sim_ui::theme::install(&ctx);
        let mut intents = Vec::new();
        let mut labels = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 800.),
                    )),
                    ..Default::default()
                },
                |ui| draw(ui, &preview, &model, &mut intents),
            );
            output.textures_delta.clear();
            labels.clear();
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    labels.push(text.galley.job.text.clone());
                }
            }
        }
        assert!(intents.is_empty());
        for expected in ["Engage route", "1  Undock", "ETA ~00:20", "FUEL EXHAUSTION"] {
            assert!(
                labels.iter().any(|label| label.contains(expected)),
                "missing {expected}: {labels:?}"
            );
        }
    }
}
