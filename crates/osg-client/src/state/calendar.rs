use bevy::prelude::*;
use std::time::Duration;

#[derive(Resource)]
pub struct CalendarClock {
    anchor: Anchor,
}

impl CalendarClock {
    pub fn new(unix_ms: i64, received_at: Duration) -> Self {
        Self {
            anchor: Anchor {
                unix_ms,
                received_at,
                correction_ms: 0,
            },
        }
    }

    pub fn observe(&mut self, calendar_unix_ms: i64, received_at: Duration) {
        let current = self.now(received_at);
        let correction = calendar_unix_ms.saturating_sub(current);
        let discontinuity = correction.unsigned_abs() > 2000;
        self.anchor = Anchor {
            unix_ms: if discontinuity {
                calendar_unix_ms
            } else {
                current
            },
            received_at,
            correction_ms: if discontinuity { 0 } else { correction },
        };
    }

    pub fn now(&self, elapsed: Duration) -> i64 {
        let anchor = &self.anchor;
        let age = elapsed.saturating_sub(anchor.received_at).as_millis();
        let age_ms = age.min(i64::MAX as u128) as i64;
        let adjustment = anchor.correction_ms.clamp(-age_ms / 20, age_ms / 20);
        anchor
            .unix_ms
            .saturating_add(age_ms)
            .saturating_add(adjustment)
    }
}

struct Anchor {
    unix_ms: i64,
    received_at: Duration,
    correction_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use osg_model::calendar::from_real_unix_ms;

    #[test]
    fn ordinary_jitter_slews_smoothly_then_returns_to_wall_clock_speed() {
        let epoch = from_real_unix_ms(1_789_689_600_000);
        let mut clock = CalendarClock::new(epoch, Duration::ZERO);
        clock.observe(epoch, Duration::ZERO);
        clock.observe(epoch + 900, Duration::from_secs(1));
        assert_eq!(clock.now(Duration::from_secs(1)), epoch + 1000);
        assert_eq!(clock.now(Duration::from_secs(2)), epoch + 1950);
        assert_eq!(clock.now(Duration::from_secs(3)), epoch + 2900);
        assert_eq!(clock.now(Duration::from_secs(60)), epoch + 59_900);

        clock.observe(epoch + 3000, Duration::from_secs(3));
        assert_eq!(clock.now(Duration::from_secs(3)), epoch + 2900);
        assert_eq!(clock.now(Duration::from_secs(4)), epoch + 3950);
        assert_eq!(clock.now(Duration::from_secs(5)), epoch + 5000);
    }

    #[test]
    fn real_wall_clock_corrections_are_adopted_in_both_directions() {
        let epoch = from_real_unix_ms(1_789_689_600_000);
        let mut clock = CalendarClock::new(epoch, Duration::ZERO);
        clock.observe(epoch, Duration::ZERO);
        clock.observe(epoch - 60_000, Duration::from_secs(1));
        assert_eq!(clock.now(Duration::from_secs(1)), epoch - 60_000);
        assert_eq!(clock.now(Duration::from_secs(2)), epoch - 59_000);
        clock.observe(epoch + 10_000, Duration::from_secs(2));
        assert_eq!(clock.now(Duration::from_secs(2)), epoch + 10_000);
        assert_eq!(clock.now(Duration::from_secs(3)), epoch + 11_000);
    }

    #[test]
    fn calendar_ignores_positive_simulation_speed_changes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(CalendarClock::new(
                from_real_unix_ms(1_789_689_600_000),
                Duration::ZERO,
            ))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        app.update();
        let epoch = from_real_unix_ms(1_789_689_600_000);
        app.world_mut()
            .resource_mut::<CalendarClock>()
            .observe(epoch, Duration::ZERO);
        {
            let mut virtual_time = app.world_mut().resource_mut::<Time<Virtual>>();
            virtual_time.set_relative_speed(0.5);
            virtual_time.set_max_delta(Duration::from_secs(2));
        }
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            1250,
        )));
        app.update();
        assert_eq!(
            app.world().resource::<Time<Virtual>>().elapsed(),
            Duration::from_millis(625)
        );
        let elapsed = app.world().resource::<Time<Real>>().elapsed();
        assert_eq!(
            app.world().resource::<CalendarClock>().now(elapsed),
            epoch + 1250
        );

        let mut virtual_time = app.world_mut().resource_mut::<Time<Virtual>>();
        virtual_time.set_relative_speed(10.);
        virtual_time.set_max_delta(Duration::from_secs(1));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            500,
        )));
        app.update();
        assert_eq!(
            app.world().resource::<Time<Virtual>>().elapsed(),
            Duration::from_millis(5625)
        );
        let elapsed = app.world().resource::<Time<Real>>().elapsed();
        assert_eq!(
            app.world().resource::<CalendarClock>().now(elapsed),
            epoch + 1750
        );
    }
}
