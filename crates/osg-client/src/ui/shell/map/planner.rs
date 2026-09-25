use super::*;
use crate::{
    assets::NavigationAccess,
    routing::{self, Plan, Progress, Status, Worker},
};
use std::sync::Arc;

#[derive(Default)]
pub struct Preview {
    context: Option<(Id, u64, u64)>,
    access_hash: Option<[u8; 32]>,
    access: Option<Handle<NavigationAccess>>,
    orders: Vec<travel::Directive>,
    preferences: travel::PlanningPreferences,
    request: Option<routing::Request>,
    loading: bool,
    commit_origin: Option<osg_model::GalacticPosition>,
    worker: Option<Worker>,
    pub progress: Arc<Progress>,
    committing: Option<Id>,
}

impl Preview {
    pub fn search_progress(&self) -> Option<&Progress> {
        (self.context.is_some() && !self.loading && self.progress.status == Status::Searching)
            .then_some(&self.progress)
    }

    #[cfg(test)]
    pub fn gallery(ship: &ShipTelemetry) -> Self {
        Self {
            progress: Arc::new(Progress {
                status: Status::Optimal,
                systems: vec![1, 1, 2, 2, 0],
                samples: vec![0, 1, 2, 3],
                explored_systems: 2,
                queued: 2,
                current: Some(1),
                branch: vec![0, 1],
                best: Some(Plan {
                    itinerary: ship.travel.itinerary.clone(),
                    estimated_loss_ppm: 300.4,
                    exotic_fuel_kg: 1.25,
                    seconds: 120.,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn fail(&mut self, message: impl ToString) {
        self.cancel();
        Arc::make_mut(&mut self.progress).status = Status::Failed(message.to_string());
    }

    pub fn cancel(&mut self) {
        if !self.loading && self.progress.status != Status::Searching {
            return;
        }
        self.loading = false;
        if let Some(worker) = &self.worker {
            worker.cancel();
            self.progress = worker.progress();
        }
        if self.progress.status == Status::Searching {
            Arc::make_mut(&mut self.progress).status = Status::Cancelled;
        }
    }

    pub fn orders(&self) -> Option<Vec<travel::Directive>> {
        self.context.map(|_| self.orders.clone())
    }

    pub fn begin(
        &mut self,
        ship: &ShipTelemetry,
        orders: Vec<travel::Directive>,
        append: bool,
        preferences: travel::PlanningPreferences,
        details: Option<&ShipPresentation>,
        server: &AssetServer,
    ) {
        let previous_orders = (self.context
            == Some((
                ship.ship,
                ship.authority_revision,
                ship.travel.directive_revision,
            )))
        .then(|| self.orders.clone());
        self.cancel();
        self.request = None;
        self.committing = None;
        self.commit_origin = None;
        self.progress = Arc::default();
        self.context = Some((
            ship.ship,
            ship.authority_revision,
            ship.travel.directive_revision,
        ));
        self.orders = if append {
            previous_orders.unwrap_or_else(|| {
                ship.travel
                    .itinerary
                    .iter()
                    .map(|entry| entry.directive.clone())
                    .collect()
            })
        } else {
            Vec::new()
        };
        self.orders.extend(orders);
        self.preferences = preferences;
        self.access_hash = details.and_then(|details| details.navigation_access);
        self.access = self
            .access_hash
            .map(|hash| server.load(crate::assets::path(hash)));
        self.loading = true;
        if self.access.is_none() {
            self.fail("Navigation access data unavailable");
        }
    }

    pub fn retry(
        &mut self,
        ship: &ShipTelemetry,
        details: Option<&ShipPresentation>,
        server: &AssetServer,
    ) {
        self.begin(
            ship,
            self.orders.clone(),
            false,
            self.preferences,
            details,
            server,
        );
    }

    pub fn update(
        &mut self,
        ship: Option<&ShipTelemetry>,
        details: Option<&ShipPresentation>,
        navigation: &crate::state::NavigationState,
        session: &crate::state::GameSession,
        assets: &Assets<NavigationAccess>,
        server: &AssetServer,
        results: &[CommandResult],
    ) {
        if self.context.is_none() {
            return;
        }
        self.commit_origin = ship.and_then(|ship| match ship.presence {
            travel::Presence::Docked { host, .. } => navigation
                .navigation
                .beacons
                .iter()
                .find(|beacon| beacon.id == host)
                .map(|beacon| beacon.pose.position),
            travel::Presence::Space => ship.pose.as_ref().map(|pose| pose.position),
            _ => None,
        });
        let context = ship.map(|ship| {
            (
                ship.ship,
                ship.authority_revision,
                ship.travel.directive_revision,
            )
        });
        if self.context != context {
            *self = Self::default();
            return;
        }
        if let Some(result) = self
            .committing
            .and_then(|id| results.iter().find(|result| result.id == id))
        {
            if let Some(error) = &result.error {
                self.fail(error);
                self.committing = None;
            } else {
                *self = Self::default();
            }
            return;
        }
        if details.and_then(|details| details.navigation_access) != self.access_hash {
            self.fail("Navigation access changed; search again");
            self.worker = None;
            self.request = None;
            return;
        }
        if self.request.as_ref().is_some_and(|request| {
            session.universe_descriptor.fingerprint != request.universe.fingerprint()
        }) {
            self.fail("Universe catalogue changed; search again");
            self.request = None;
            return;
        }
        if !matches!(self.progress.status, Status::Failed(_)) {
            if let Some(worker) = &self.worker {
                self.progress = worker.progress();
            }
        }
        if !self.loading {
            return;
        }
        let Some(handle) = &self.access else {
            return;
        };
        if matches!(
            server.load_state(handle.id()),
            bevy::asset::LoadState::Failed(_)
        ) {
            self.fail("Navigation access could not be loaded; search again");
            return;
        }
        let Some(access) = assets.get(handle) else {
            return;
        };
        let Some(ship) = ship else {
            return;
        };
        let Some(details) = details else {
            return;
        };
        let input = (|| -> anyhow::Result<routing::Request> {
            let universe = session.universe.clone();
            anyhow::ensure!(
                matches!(
                    ship.presence,
                    travel::Presence::Space | travel::Presence::Docked { .. }
                ),
                "Wait for slip arrival before planning"
            );
            let origin = match ship.presence {
                travel::Presence::Docked { host, .. } => navigation
                    .navigation
                    .beacons
                    .iter()
                    .find(|beacon| beacon.id == host)
                    .map(|beacon| beacon.pose.position),
                _ => ship.pose.as_ref().map(|pose| pose.position),
            }
            .ok_or_else(|| anyhow::anyhow!("Departure position unavailable"))?;
            Ok(routing::Request {
                universe,
                origin,
                current_system: ship.location.system,
                mass_kg: details.mass_kg,
                exotic_kg: details
                    .slip_exotic_fuel_kg
                    .ok_or_else(|| anyhow::anyhow!("Exotic fuel telemetry unavailable"))?,
                preferences: self.preferences,
                directives: self.orders.clone(),
                assisted: access.0.clone(),
                stations: navigation
                    .navigation
                    .beacons
                    .iter()
                    .filter_map(|beacon| beacon.systems.first().map(|system| (beacon.id, *system)))
                    .collect(),
            })
        })();
        match input {
            Ok(request) => {
                let worker = self.worker.get_or_insert_with(Worker::default);
                worker.start(request.clone());
                self.request = Some(request);
                self.loading = false;
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn sent_commit(&mut self, id: Id) {
        self.cancel();
        self.committing = Some(id);
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.progress.best.as_ref()
    }

    pub fn commit(&self, ship: &ShipTelemetry, details: &ShipPresentation) -> Option<ShipCommand> {
        if !details.mass_kg.is_finite()
            || details.mass_kg <= 0.
            || !details.slip_exotic_fuel_kg?.is_finite()
        {
            return None;
        }
        if !matches!(
            ship.presence,
            travel::Presence::Space | travel::Presence::Docked { .. }
        ) || self.committing.is_some()
            || self.context
                != Some((
                    ship.ship,
                    ship.authority_revision,
                    ship.travel.directive_revision,
                ))
            || self.access_hash != details.navigation_access
        {
            return None;
        }
        let plan = self.plan()?;
        let input = self.request.as_ref()?;
        if input.current_system != ship.location.system {
            return None;
        }
        let mut origin = self.commit_origin?;
        let mut fuel = 0.;
        let mut loss = 0.;
        for entry in &plan.itinerary {
            if let travel::Directive::SlipToSystem(id) = entry.directive {
                let target = &input.universe.systems()[input.universe.system_index(id.0)?];
                let distance = target.position.relative_to(origin).length();
                fuel +=
                    travel::slip::exotic_fuel_kg(details.mass_kg, distance / travel::slip::LY_M);
                loss += travel::slip::log_loss_from_ppm(travel::slip::capture_loss_ppm(
                    travel::slip::exclusion_radius_m(target.stellar_mass),
                    distance,
                    input.assisted.contains(&id),
                ));
                origin = target.position;
            }
        }
        if fuel > details.slip_exotic_fuel_kg? * self.preferences.fuel_fraction
            || loss > travel::slip::log_loss_from_ppm(self.preferences.max_loss_ppm)
        {
            return None;
        }
        Some(ShipCommand::SetItinerary {
            preferences: self.preferences,
            engage: true,
            expected_revision: ship.travel.directive_revision,
            itinerary: plan
                .itinerary
                .iter()
                .map(|entry| entry.directive.clone())
                .collect(),
        })
    }
}

pub fn draw(ui: &mut egui::Ui, preview: &Preview, model: &FrameModel, intents: &mut Vec<Intent>) {
    if preview.context.is_none() && preview.plan().is_none() {
        ui.weak("Choose a destination to search for a route.");
        return;
    }
    ui.separator();
    ui.strong("ROUTE PREVIEW");
    if preview.loading {
        ui.spinner();
        ui.label("Loading navigation access…");
    } else {
        ui.label(match &preview.progress.status {
            Status::Searching if preview.plan().is_some() => "Best route so far · searching",
            Status::Searching => "Searching",
            Status::Optimal => "Fastest estimated route",
            Status::Cancelled => "Cancelled",
            Status::Exhausted(reason) => reason,
            Status::Failed(error) => error,
        });
    }
    if let Some(progress) = preview.search_progress() {
        ui.small(format!(
            "{:.1}s · {} explored · {} queued",
            progress.elapsed.as_secs_f64(),
            progress.explored_systems,
            progress.queued
        ));
    }
    if preview.loading || preview.progress.status == Status::Searching {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        if ui.button("Cancel search").clicked() {
            intents.push(Intent::CancelRoute);
        }
    } else if ui.button("Search again").clicked() {
        intents.push(Intent::RetryRoute);
    }
    if let Some(plan) = preview.plan() {
        let can_commit = model.connected
            && model
                .ship
                .zip(model.details)
                .is_some_and(|(ship, details)| preview.commit(ship, details).is_some());
        if ui
            .add_enabled(can_commit, egui::Button::new("Engage best route"))
            .clicked()
        {
            intents.push(Intent::CommitRoute);
        }
        if !can_commit && preview.committing.is_none() && preview.context.is_some() {
            ui.weak("Current ship state changed; search again before engaging.");
        }
        ui.label(format!(
            "{} directives · estimated {:.0}s",
            plan.itinerary.len(),
            plan.seconds
        ));
        ui.label(format!("Exotic fuel: {:.3} kg", plan.exotic_fuel_kg));
        ui.label(format!(
            "Trip loss: {:.3} ppm / {:.3} ppm",
            plan.estimated_loss_ppm, preview.preferences.max_loss_ppm
        ));
        ui.small(
            "Catalogue estimates. The flight computer plans local maneuvers during execution.",
        );
        for (index, entry) in plan.itinerary.iter().enumerate() {
            ui.label(format!("{}  {}", index + 1, entry.label));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_overlay_exists_only_during_an_active_search() {
        let mut preview = Preview::default();
        assert!(preview.search_progress().is_none());
        preview.context = Some((Id::new(), 0, 0));
        assert!(preview.search_progress().is_some());
        preview.loading = true;
        assert!(preview.search_progress().is_none());
        preview.loading = false;
        for status in [
            Status::Optimal,
            Status::Cancelled,
            Status::Exhausted("No arrival".into()),
            Status::Failed("Test error".into()),
        ] {
            Arc::make_mut(&mut preview.progress).status = status;
            assert!(preview.search_progress().is_none());
        }
    }

    #[test]
    fn engaging_rechecks_resources_authority_and_revision_and_sends_only_directives() {
        let universe = Arc::new(
            osg_universe::universe::Universe::init(osg_universe::example_config()).unwrap(),
        );
        let target = Id(universe.systems()[0].id);
        let origin = universe.systems()[0]
            .position
            .offset_by(glam::DVec3::X * travel::slip::LY_M);
        let mut ship = crate::ui::tests::ship(Id::new()).0;
        ship.presence = travel::Presence::Space;
        ship.location.system = None;
        let mut details = crate::ui::console::tests::details();
        details.ship = ship.ship;
        details.mass_kg = 100_000.;
        details.slip_exotic_fuel_kg = Some(10.);
        details.navigation_access = Some([1; 32]);
        let preferences = travel::PlanningPreferences {
            fuel_fraction: 1.,
            max_loss_ppm: 1_000_000.,
            allow_slipdrive: true,
        };
        let mut preview = Preview {
            context: Some((
                ship.ship,
                ship.authority_revision,
                ship.travel.directive_revision,
            )),
            access_hash: details.navigation_access,
            commit_origin: Some(origin),
            preferences,
            request: Some(routing::Request {
                universe,
                origin,
                current_system: None,
                mass_kg: details.mass_kg,
                exotic_kg: 10.,
                preferences,
                directives: vec![travel::Directive::SlipToSystem(target)],
                assisted: Default::default(),
                stations: Default::default(),
            }),
            progress: Arc::new(Progress {
                best: Some(Plan {
                    itinerary: vec![travel::ItineraryEntry {
                        directive: travel::Directive::SlipToSystem(target),
                        label: "Test".into(),
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert!(matches!(
            preview.commit(&ship, &details),
            Some(ShipCommand::SetItinerary { engage: true, .. })
        ));
        details.slip_exotic_fuel_kg = Some(0.);
        assert!(preview.commit(&ship, &details).is_none());
        details.slip_exotic_fuel_kg = Some(10.);
        details.mass_kg *= 100.;
        assert!(preview.commit(&ship, &details).is_none());
        details.mass_kg /= 100.;
        ship.authority_revision += 1;
        assert!(preview.commit(&ship, &details).is_none());
        ship.authority_revision -= 1;
        ship.travel.directive_revision += 1;
        assert!(preview.commit(&ship, &details).is_none());
        ship.travel.directive_revision -= 1;
        details.navigation_access = None;
        assert!(preview.commit(&ship, &details).is_none());
        details.navigation_access = preview.access_hash;
        preview.commit_origin = None;
        assert!(preview.commit(&ship, &details).is_none());
    }

    #[test]
    fn cancelling_metadata_load_retains_a_retryable_preview() {
        let mut preview = Preview {
            loading: true,
            ..Default::default()
        };
        preview.cancel();
        assert!(!preview.loading);
        assert_eq!(preview.progress.status, Status::Cancelled);
        preview.fail("Access changed");
        preview.cancel();
        assert_eq!(
            preview.progress.status,
            Status::Failed("Access changed".into())
        );
    }
}
