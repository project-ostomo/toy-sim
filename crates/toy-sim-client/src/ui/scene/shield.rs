use super::{RenderSource, Shield};
use crate::state::{CombatPublication, Optical, RenderTime};
use bevy::prelude::*;
use std::collections::HashMap;
use toy_sim_model::presentation::CombatEventKind;
use toy_sim_ship_view::thermal::ThermalSphere;

const FLASH_TEMPERATURE_K: f32 = 3000.;
const LIFETIME_NS: u64 = 4_000_000_000;

fn temperature_boost(event_ns: u64, now_ns: u64) -> f32 {
    let Some(age_ns) = now_ns.checked_sub(event_ns) else {
        return 0.;
    };
    if age_ns >= LIFETIME_NS {
        return 0.;
    }
    FLASH_TEMPERATURE_K * (0.05_f32.ln() * (age_ns as f32 * 1e-9)).exp()
}

pub(super) fn update_flashes(
    clock: Res<RenderTime>,
    publications: Query<&CombatPublication>,
    contacts: Query<(Option<&Optical>, Option<&super::glints::VisualContact>)>,
    ships: Query<&RenderSource>,
    mut shields: Query<(&ChildOf, &mut ThermalSphere), With<Shield>>,
) {
    let mut boosts = HashMap::<_, f32>::new();
    for publication in &publications {
        let event = &publication.0;
        let CombatEventKind::Impact {
            target: Some(target),
            shield: true,
            ..
        } = &event.kind
        else {
            continue;
        };
        let boost = temperature_boost(event.sim_time_ns, clock.display_ns);
        if boost > 0. {
            let current = boosts.entry((target.group, target.track)).or_default();
            *current = current.max(boost);
        }
    }
    for (parent, mut shield) in &mut shields {
        let Some(reference) = ships
            .get(parent.parent())
            .ok()
            .and_then(|source| contacts.get(source.0).ok())
            .and_then(|(contact, visual)| {
                contact
                    .and_then(|c| c.0.contact)
                    .or_else(|| visual.and_then(|v| v.0))
            })
        else {
            continue;
        };
        if let Some(boost) = boosts.get(&(reference.group, reference.track)) {
            shield.temperature_k += boost;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperature_flash_fades_to_five_percent_in_one_second() {
        let event_ns = 1_000_000_000;
        assert_eq!(temperature_boost(event_ns, event_ns - 1), 0.);
        assert_eq!(temperature_boost(event_ns, event_ns), 3000.);
        assert!((temperature_boost(event_ns, event_ns + 1_000_000_000) - 150.).abs() < 0.001);
        assert!((temperature_boost(event_ns, event_ns + 2_000_000_000) - 7.5).abs() < 0.001);
        assert_eq!(temperature_boost(event_ns, event_ns + LIFETIME_NS), 0.);
    }
}
