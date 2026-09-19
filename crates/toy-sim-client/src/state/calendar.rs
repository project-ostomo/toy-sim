use bevy::prelude::*;
use std::time::Duration;

#[derive(Resource, Default)]
pub(crate) struct CalendarClock {
    anchor: Option<Anchor>,
}

struct Anchor {
    unix_ms: i64,
    received_at: Duration,
    correction_ms: i64,
}

impl CalendarClock {
    pub fn observe(&mut self, calendar_unix_ms: i64, received_at: Duration) {
        let current = self.now(received_at).unwrap_or(calendar_unix_ms);
        let correction = calendar_unix_ms.saturating_sub(current);
        let discontinuity = correction.unsigned_abs() > 2000;
        self.anchor = Some(Anchor {
            unix_ms: if discontinuity {
                calendar_unix_ms
            } else {
                current
            },
            received_at,
            correction_ms: if discontinuity { 0 } else { correction },
        });
    }

    pub fn now(&self, elapsed: Duration) -> Option<i64> {
        self.anchor.as_ref().map(|anchor| {
            let age = elapsed.saturating_sub(anchor.received_at).as_millis();
            let age_ms = age.min(i64::MAX as u128) as i64;
            let adjustment = anchor.correction_ms.clamp(-age_ms / 20, age_ms / 20);
            anchor
                .unix_ms
                .saturating_add(age_ms)
                .saturating_add(adjustment)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use toy_sim_model::calendar::from_real_unix_ms;

    #[test]
    fn ordinary_jitter_slews_smoothly_then_returns_to_wall_clock_speed() {
        let mut clock = CalendarClock::default();
        let epoch = from_real_unix_ms(1_789_689_600_000);
        assert_eq!(clock.now(Duration::ZERO), None);
        clock.observe(epoch, Duration::ZERO);
        clock.observe(epoch + 900, Duration::from_secs(1));
        assert_eq!(clock.now(Duration::from_secs(1)), Some(epoch + 1000));
        assert_eq!(clock.now(Duration::from_secs(2)), Some(epoch + 1950));
        assert_eq!(clock.now(Duration::from_secs(3)), Some(epoch + 2900));
        assert_eq!(clock.now(Duration::from_secs(60)), Some(epoch + 59_900));

        clock.observe(epoch + 3000, Duration::from_secs(3));
        assert_eq!(clock.now(Duration::from_secs(3)), Some(epoch + 2900));
        assert_eq!(clock.now(Duration::from_secs(4)), Some(epoch + 3950));
        assert_eq!(clock.now(Duration::from_secs(5)), Some(epoch + 5000));
    }

    #[test]
    fn real_wall_clock_corrections_are_adopted_in_both_directions() {
        let mut clock = CalendarClock::default();
        let epoch = from_real_unix_ms(1_789_689_600_000);
        clock.observe(epoch, Duration::ZERO);
        clock.observe(epoch - 60_000, Duration::from_secs(1));
        assert_eq!(clock.now(Duration::from_secs(1)), Some(epoch - 60_000));
        assert_eq!(clock.now(Duration::from_secs(2)), Some(epoch - 59_000));
        clock.observe(epoch + 10_000, Duration::from_secs(2));
        assert_eq!(clock.now(Duration::from_secs(2)), Some(epoch + 10_000));
        assert_eq!(clock.now(Duration::from_secs(3)), Some(epoch + 11_000));
    }

    #[test]
    fn calendar_advances_during_simulation_pause_and_ignores_simulation_speed() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CalendarClock>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        app.update();
        let epoch = from_real_unix_ms(1_789_689_600_000);
        app.world_mut()
            .resource_mut::<CalendarClock>()
            .observe(epoch, Duration::ZERO);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            1250,
        )));
        app.update();
        assert_eq!(
            app.world().resource::<Time<Virtual>>().elapsed(),
            Duration::ZERO
        );
        let elapsed = app.world().resource::<Time<Real>>().elapsed();
        assert_eq!(
            app.world().resource::<CalendarClock>().now(elapsed),
            Some(epoch + 1250)
        );

        let mut virtual_time = app.world_mut().resource_mut::<Time<Virtual>>();
        virtual_time.unpause();
        virtual_time.set_relative_speed(10.);
        virtual_time.set_max_delta(Duration::from_secs(1));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            500,
        )));
        app.update();
        assert_eq!(
            app.world().resource::<Time<Virtual>>().elapsed(),
            Duration::from_secs(5)
        );
        let elapsed = app.world().resource::<Time<Real>>().elapsed();
        assert_eq!(
            app.world().resource::<CalendarClock>().now(elapsed),
            Some(epoch + 1750)
        );
    }
}
