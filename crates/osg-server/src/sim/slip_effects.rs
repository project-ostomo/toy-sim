//! Shared residual slip light, independent of the ship's lifetime and sensors.
use super::{identity, simulation::SimulationCounters, spatial, travel};
use anyhow::{Result, ensure};
use bevy::{math::DVec3, prelude::*};
use osg_model::{AccountId, GalacticPosition, Id, ViewState, slip_visual::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct Span {
    source: Id,
    wake: SlipWake,
}

#[derive(Resource, Clone, Default, Serialize, Deserialize)]
pub struct SlipHistory {
    spans: Vec<Span>,
    transitions: Vec<SlipTransition>,
}

fn now(world: &World) -> u64 {
    world.resource::<SimulationCounters>().ticks * osg_model::TICK_NS
}

fn radius(world: &World, ship: Entity) -> f64 {
    world
        .get::<super::vessel::ShipDesign>(ship)
        .map_or(10.0, |d| d.0.radius)
}

pub fn record_transition(
    world: &mut World,
    ship: Entity,
    position: GalacticPosition,
    drift_m_s: [f64; 3],
    direction: [f64; 3],
    arriving: bool,
    time_ns: u64,
) {
    let transition = SlipTransition {
        view: 0,
        id: Id::new(),
        time_ns,
        position,
        drift_m_s,
        direction,
        radius_m: radius(world, ship),
        arriving,
        seed: rand::random(),
    };
    world.init_resource::<SlipHistory>();
    world
        .resource_mut::<SlipHistory>()
        .transitions
        .push(transition);
}

pub fn record_span(
    world: &mut World,
    ship: Entity,
    start: GalacticPosition,
    end: GalacticPosition,
    start_ns: u64,
    end_ns: u64,
    drift_m_s: [f64; 3],
) {
    if end_ns <= start_ns || start == end {
        return;
    }
    let Some(source) = world.get::<identity::Identity>(ship).map(|id| id.0) else {
        return;
    };
    let radius_m = radius(world, ship);
    world.init_resource::<SlipHistory>();
    let mut history = world.resource_mut::<SlipHistory>();
    if let Some(previous) = history
        .spans
        .iter_mut()
        .rev()
        .find(|span| span.source == source)
    {
        let wake = &mut previous.wake;
        let old_velocity =
            wake.end.relative_to(wake.start) / ((wake.end_ns - wake.start_ns) as f64 * 1e-9);
        let velocity = end.relative_to(start) / ((end_ns - start_ns) as f64 * 1e-9);
        if wake.end == start
            && wake.end_ns == start_ns
            && wake.drift_m_s == drift_m_s
            && (old_velocity - velocity).length() < velocity.length() * 1e-9
        {
            wake.end = end;
            wake.end_ns = end_ns;
            return;
        }
    }
    history.spans.push(Span {
        source,
        wake: SlipWake {
            view: 0,
            id: Id::new(),
            start,
            end,
            start_ns,
            end_ns,
            drift_m_s,
            radius_m,
            seed: rand::random(),
            offset_m: 0.0,
        },
    });
}

fn clip(wake: &SlipWake, a: f64, b: f64) -> SlipWake {
    let mut clipped = wake.clone();
    let delta = wake.end.relative_to(wake.start);
    let duration = (wake.end_ns - wake.start_ns) as f64;
    clipped.start = wake.start.offset_by(delta * a);
    clipped.end = wake.start.offset_by(delta * b);
    clipped.start_ns = wake.start_ns + (duration * a).round() as u64;
    clipped.end_ns = wake.start_ns + (duration * b).round() as u64;
    clipped.offset_m += delta.length() * a;
    clipped
}

impl SlipHistory {
    pub(crate) fn push_transition(&mut self, transition: SlipTransition) {
        self.transitions.push(transition);
    }

    pub fn prune_at(&mut self, time_ns: u64) {
        let cutoff = time_ns.saturating_sub((WAKE_LIFETIME_S * 1e9) as u64);
        self.spans.retain_mut(|span| {
            let wake = &mut span.wake;
            if wake.end_ns <= cutoff {
                return false;
            }
            if wake.start_ns < cutoff {
                let a = (cutoff - wake.start_ns) as f64 / (wake.end_ns - wake.start_ns) as f64;
                *wake = clip(wake, a, 1.0);
            }
            true
        });
        self.transitions.retain(|event| {
            time_ns.saturating_sub(event.time_ns) < (TRANSITION_LIFETIME_S * 1e9) as u64
        });
    }

    pub fn validate(&self, time_ns: u64) -> Result<()> {
        let finite = |v: [f64; 3]| v.iter().all(|v| v.is_finite());
        for span in &self.spans {
            let w = &span.wake;
            ensure!(
                w.start_ns < w.end_ns && w.end_ns <= time_ns + osg_model::TICK_NS,
                "invalid wake times"
            );
            ensure!(
                finite(w.drift_m_s) && w.radius_m.is_finite() && w.radius_m > 0.0,
                "invalid wake geometry"
            );
            ensure!(
                w.offset_m.is_finite() && w.offset_m >= 0.0,
                "invalid wake offset"
            );
        }
        for event in &self.transitions {
            ensure!(
                event.time_ns <= time_ns + osg_model::TICK_NS,
                "invalid slip transition time"
            );
            ensure!(
                finite(event.drift_m_s)
                    && finite(event.direction)
                    && (DVec3::from_array(event.direction).length() - 1.0).abs() < 1e-6
                    && event.radius_m.is_finite()
                    && event.radius_m > 0.0,
                "invalid slip transition geometry"
            );
        }
        Ok(())
    }
}

pub fn prune(clock: Res<SimulationCounters>, history: Option<ResMut<SlipHistory>>) {
    if let Some(mut history) = history {
        history.prune_at(clock.ticks * osg_model::TICK_NS);
    }
}

fn blocked(
    world: &World,
    observer: Entity,
    origin: GalacticPosition,
    target: GalacticPosition,
) -> bool {
    let Some(index) = world.get_resource::<spatial::SpatialIndex>() else {
        return false;
    };
    let displacement = target.relative_to(origin);
    index.any_optical_blocker_on_segment(origin, displacement, |i| {
        let object = index.objects[i];
        object.entity != observer
            && spatial::sphere_blocks(
                displacement,
                object.position.relative_to(origin),
                object.radius_m,
            )
    })
}

/// Split silhouettes before publication; the renderer also depth-tests against local geometry.
fn visible_pieces(
    world: &World,
    observer: Entity,
    origin: GalacticPosition,
    wake: &SlipWake,
    time: u64,
    depth: u8,
    out: &mut Vec<SlipWake>,
) {
    let samples = [0.0, 0.5, 1.0].map(|f| blocked(world, observer, origin, wake.position(f, time)));
    if samples.iter().all(|&v| v) {
        return;
    }
    if samples.iter().all(|&v| !v) {
        out.push(wake.clone());
    } else if depth < 7 {
        visible_pieces(
            world,
            observer,
            origin,
            &clip(wake, 0.0, 0.5),
            time,
            depth + 1,
            out,
        );
        visible_pieces(
            world,
            observer,
            origin,
            &clip(wake, 0.5, 1.0),
            time,
            depth + 1,
            out,
        );
    }
}

pub fn observe(world: &World, account: AccountId, views: &[ViewState]) -> SlipPresentation {
    let mut result = SlipPresentation::default();
    let Some(history) = world.get_resource::<SlipHistory>() else {
        return result;
    };
    let time = now(world);
    for view in views {
        let Some(observer) = view
            .focused_ship
            .and_then(|id| super::commands::observe(world, account, id).ok())
        else {
            continue;
        };
        let slipping = world
            .get::<travel::PresenceState>(observer)
            .is_some_and(|presence| {
                matches!(presence.0, osg_model::travel::Presence::SlipTransit(_))
            });
        if world.get::<travel::Dormant>(observer).is_some() && !slipping {
            continue;
        }
        // The client interpolates behind the newest pose. Keep enough of the
        // ship's own wake to cover that interval even at interstellar speeds.
        let range = world
            .get::<travel::Transit>(observer)
            .map_or(WAKE_VIEW_RANGE_M, |transit| {
                WAKE_VIEW_RANGE_M.max(transit.speed_ly_s * osg_model::travel::slip::LY_M * 0.2)
            });
        let mut candidates = Vec::new();
        for span in &history.spans {
            // A ship carries its own freshly formed wake into the slip view.
            if slipping && Some(span.source) != view.focused_ship {
                continue;
            }
            let wake = &span.wake;
            if time.saturating_sub(wake.end_ns) >= (WAKE_LIFETIME_S * 1e9) as u64 {
                continue;
            }
            let a = wake.position(0.0, time).relative_to(view.origin);
            let delta = wake.position(1.0, time).relative_to(view.origin) - a;
            let length = delta.length();
            if length < 0.001 {
                continue;
            }
            let axis = delta / length;
            let along = -a.dot(axis);
            let distance2 = (a + axis * along.clamp(0.0, length)).length_squared();
            if distance2 > range.powi(2) {
                continue;
            }
            let lateral2 = (a + axis * along).length_squared();
            let reach = (range.powi(2) - lateral2).max(0.0).sqrt();
            let lo = ((along - reach) / length).clamp(0.0, 1.0);
            let hi = ((along + reach) / length).clamp(0.0, 1.0);
            if hi <= lo {
                continue;
            }
            let age = time.saturating_sub(wake.end_ns) as f64 * 1e-9;
            let flux = osg_model::optical::flux_w_m2(
                1e8 * wake.radius_m.powi(2) * wake_envelope(age),
                distance2.sqrt(),
                wake.radius_m,
            );
            if flux < osg_model::optical::MIN_OPTICAL_FLUX_W_M2 {
                continue;
            }
            candidates.push((flux, clip(wake, lo, hi)));
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
        let mut visible = Vec::new();
        for (_, wake) in candidates.into_iter().take(MAX_WAKE_SPANS_PER_VIEW) {
            visible_pieces(world, observer, view.origin, &wake, time, 0, &mut visible);
            if visible.len() >= MAX_WAKE_SPANS_PER_VIEW {
                break;
            }
        }
        result.wakes.extend(
            visible
                .into_iter()
                .take(MAX_WAKE_SPANS_PER_VIEW)
                .map(|mut w| {
                    w.view = view.id;
                    // Only the periodic noise phase is public. The full offset
                    // would reveal a departure point outside this view.
                    w.offset_m %= 4096.0 * w.radius_m.max(8.0);
                    w
                }),
        );
        if slipping {
            continue;
        }
        let mut transitions: Vec<_> = history
            .transitions
            .iter()
            .filter_map(|event| {
                let age = time.saturating_sub(event.time_ns) as f64 * 1e-9;
                let position = event
                    .position
                    .offset_by(DVec3::from_array(event.drift_m_s) * age);
                let distance = position.relative_to(view.origin).length();
                (age < TRANSITION_LIFETIME_S
                    && distance <= WAKE_VIEW_RANGE_M
                    && !blocked(world, observer, view.origin, position))
                .then_some((distance, event))
            })
            .collect();
        transitions.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
        result
            .transitions
            .extend(transitions.into_iter().take(16).map(|(_, event)| {
                let mut event = event.clone();
                event.view = view.id;
                event
            }));
    }
    result
}

#[cfg(test)]
mod tests;
