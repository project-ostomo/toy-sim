use super::*;
use crate::sim::routing::{RouteEnvironment, SystemTarget};
use anyhow::Context;
use osg_model::travel::Directive;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Environment {
    source: Arc<ShipScan<'static>>,
    cancel: Arc<AtomicBool>,
    deadline: std::time::Instant,
}

impl Environment {
    fn check_budget(&self) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "route computation cancelled"
        );
        ensure!(
            std::time::Instant::now() < self.deadline,
            "route search time budget exhausted"
        );
        Ok(())
    }

    pub(crate) fn capture(
        world: &mut World,
        ship: Entity,
        cancel: Arc<AtomicBool>,
        _input: &crate::sim::routing::RouteRequest,
    ) -> Result<Self> {
        let source = Arc::new(
            super::current_ship_source(world, ship)
                .context("route observations unavailable")?
                .without_sensors(),
        );
        Ok(Self {
            source,
            cancel,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        })
    }

    pub(crate) fn prepare(&mut self) -> Result<()> {
        self.deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        self.check_budget()
    }

    fn target(&self, index: usize) -> Result<SystemTarget> {
        self.check_budget()?;
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        let system = &universe.systems[index];
        Ok(SystemTarget {
            id: Id(system.id),
            position: system.position,
            influence_m: system.influence_bound,
        })
    }
}

impl RouteEnvironment for Environment {
    fn system(&self, id: Id) -> Result<SystemTarget> {
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        self.target(universe.system_index(id.0).context("unknown system")?)
    }

    fn containing(&self, position: GalacticPosition) -> Result<Option<Id>> {
        self.check_budget()?;
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        Ok(universe
            .index
            .containing_segment(position, DVec3::ZERO)
            .into_iter()
            .filter_map(|index| {
                let system = &universe.systems[index];
                let normalized = system.position.relative_to(position).length()
                    / system.influence_bound.max(1.0);
                (normalized <= 1.0).then_some((normalized, Id(system.id)))
            })
            .min_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
            })
            .map(|(_, id)| id))
    }

    fn station_system(&self, id: Id) -> Result<Id> {
        self.check_budget()?;
        let beacon = self
            .source
            .beacons
            .get(&id)
            .context("public station unavailable")?;
        ensure!(!beacon.bays.is_empty(), "station has no docking bays");
        let visible = self.source.beacon(beacon);
        ensure!(!visible.bays.is_empty(), "no accessible docking berth");
        self.containing(visible.pose.position)?
            .context("station is outside a known system")
    }

    fn candidates(
        &self,
        origin: GalacticPosition,
        goal: GalacticPosition,
        limit: usize,
    ) -> Result<Vec<SystemTarget>> {
        let universe = &self
            .source
            .universe
            .as_ref()
            .context("universe unavailable")?
            .registry
            .universe;
        let displacement = goal.relative_to(origin);
        let mut indices = Vec::new();
        for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
            indices.extend(
                universe
                    .index
                    .nearest_many(origin.offset_by(displacement * fraction), limit / 5 + 1),
            );
        }
        indices.sort_unstable();
        indices.dedup();
        indices
            .into_iter()
            .map(|index| self.target(index))
            .collect()
    }

    fn label(&self, directive: &Directive) -> String {
        let name = match directive {
            Directive::SlipToSystem(id) => self.source.universe.as_ref().and_then(|source| {
                let universe = &source.registry.universe;
                universe
                    .system_index(id.0)
                    .map(|index| universe.systems[index].name.to_string())
            }),
            Directive::DockAt(id) => self
                .source
                .beacons
                .get(id)
                .and_then(|beacon| beacon.beacon.iff.labels.iter().next())
                .cloned(),
        };
        name.map_or_else(
            || directive.label(),
            |name| match directive {
                Directive::SlipToSystem(_) => format!("Slip · {name}"),
                Directive::DockAt(_) => format!("Dock · {name}"),
            },
        )
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
