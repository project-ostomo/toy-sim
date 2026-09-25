use glam::{DQuat, DVec3};
use osg_model::{GalacticPosition, LocalObstacle, LocalSpace, Pose, travel::Target};

#[derive(Default)]
pub struct Avoidance {
    waypoint: Option<Waypoint>,
    side: Option<(Target, DVec3)>,
}

struct Waypoint {
    reference: Target,
    offset: DVec3,
}

pub struct Steering {
    pub target: Pose,
    pub changed: bool,
    pub detouring: bool,
}

impl Avoidance {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn steer(
        &mut self,
        pose: &Pose,
        destination: &Pose,
        space: &LocalSpace,
        own_radius: f64,
        ignored: Option<&Target>,
    ) -> Steering {
        if space.truncated {
            let mut target = pose.clone();
            if let Some(obstacle) = space.obstacles.first() {
                target.velocity = obstacle.pose.velocity;
            }
            return Steering {
                target,
                changed: self.waypoint.take().is_some(),
                detouring: true,
            };
        }

        let relevant = |obstacle: &&LocalObstacle| ignored != Some(&obstacle.reference);
        let clear = |target: GalacticPosition| {
            space.obstacles.iter().filter(relevant).all(|obstacle| {
                intersection(
                    pose.position,
                    target,
                    obstacle.pose.position,
                    obstacle.radius_m + own_radius + 5.,
                )
                .is_none()
            })
        };
        let blocker = space
            .obstacles
            .iter()
            .filter(relevant)
            .filter_map(|obstacle| {
                let radius = obstacle.radius_m + own_radius + 5.;
                intersection(
                    pose.position,
                    destination.position,
                    obstacle.pose.position,
                    radius,
                )
                .map(|distance| (distance, obstacle, radius))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));

        let Some((_, obstacle, radius)) = blocker else {
            let changed = self.waypoint.take().is_some();
            self.side = None;
            return Steering {
                target: destination.clone(),
                changed,
                detouring: false,
            };
        };

        if let Some(waypoint) = &self.waypoint
            && waypoint.reference == obstacle.reference
        {
            let target = offset_pose(&obstacle.pose, waypoint.offset);
            if target.position.relative_to(pose.position).length() > 10. && clear(target.position) {
                return Steering {
                    target,
                    changed: false,
                    detouring: true,
                };
            }
        }

        let relative = pose.position.relative_to(obstacle.pose.position);
        let radial = relative.try_normalize().unwrap_or(DVec3::Z);
        let destination_direction = destination
            .position
            .relative_to(obstacle.pose.position)
            .normalize_or_zero();
        let path_radius = radius + (radius * 0.4).max(100.);

        let offset = if relative.length() < path_radius - 10. {
            radial * path_radius
        } else {
            let plane = self
                .side
                .as_ref()
                .filter(|(reference, _)| reference == &obstacle.reference)
                .map(|(_, plane)| *plane)
                .unwrap_or_else(|| {
                    radial
                        .cross(destination_direction)
                        .try_normalize()
                        .unwrap_or_else(|| radial.any_orthonormal_vector())
                });
            self.side = Some((obstacle.reference.clone(), plane));

            let maximum_angle = 2. * (radius / path_radius).acos();
            let angle = (maximum_angle * 0.4).min(std::f64::consts::FRAC_PI_4);
            DQuat::from_axis_angle(plane, angle) * radial * path_radius
        };

        let target = offset_pose(&obstacle.pose, offset);
        if !clear(target.position) {
            self.waypoint = None;
            let mut target = pose.clone();
            target.velocity = obstacle.pose.velocity;
            return Steering {
                target,
                changed: true,
                detouring: true,
            };
        }

        self.waypoint = Some(Waypoint {
            reference: obstacle.reference.clone(),
            offset,
        });
        Steering {
            target,
            changed: true,
            detouring: true,
        }
    }
}

fn intersection(
    from: GalacticPosition,
    to: GalacticPosition,
    center: GalacticPosition,
    radius: f64,
) -> Option<f64> {
    let start = from.relative_to(center);
    let segment = to.relative_to(from);
    let length = segment.length();
    if length <= 1e-6 {
        return None;
    }
    if start.length() <= radius && start.dot(segment) >= 0. {
        return None;
    }

    let direction = segment / length;
    let along = -start.dot(direction);
    let nearest = along.clamp(0., length);
    if (start + direction * nearest).length_squared() >= radius * radius {
        return None;
    }
    let perpendicular = start.length_squared() - along * along;
    Some((along - (radius * radius - perpendicular).max(0.).sqrt()).max(0.))
}

pub fn offset_pose(pose: &Pose, offset: DVec3) -> Pose {
    let mut result = pose.clone();
    result.position = result.position.offset_by(offset);
    result
}

pub fn outside_exclusions(
    position: GalacticPosition,
    preferred_direction: DVec3,
    space: &LocalSpace,
    own_radius: f64,
) -> Option<Pose> {
    if space.truncated {
        return None;
    }
    let mut candidate = position;
    let mut velocity = [0.; 3];
    let margin = (own_radius * 0.1).max(10.);
    let destination = position.offset_by(preferred_direction);
    for _ in 0..space.obstacles.len().saturating_mul(6).max(1) {
        let Some(obstacle) = space.obstacles.iter().find(|obstacle| {
            obstacle.slip_exclusion_m > 0.
                && candidate.relative_to(obstacle.pose.position).length()
                    <= obstacle.slip_exclusion_m + own_radius
        }) else {
            // Clear the outgoing segment as well as the departure position. The
            // intended target's sphere is allowed at the end of that segment.
            let direction = destination.relative_to(candidate).try_normalize()?;
            let departure_body = space.departure_body(candidate, 0.);
            let blocker = space.obstacles.iter().find(|obstacle| {
                let ignored = departure_body == Some(&obstacle.reference);
                let radius = if ignored {
                    obstacle.radius_m
                } else {
                    obstacle.slip_exclusion_m.max(obstacle.radius_m)
                } + own_radius;
                obstacle.slip_exclusion_m > 0.
                    && destination.relative_to(obstacle.pose.position).length() > radius
                    && intersection(candidate, destination, obstacle.pose.position, radius)
                        .is_some()
            });
            let Some(blocker) = blocker else {
                return Some(Pose {
                    position: candidate,
                    velocity,
                    ..Default::default()
                });
            };
            velocity = blocker.pose.velocity;
            let relative = candidate.relative_to(blocker.pose.position);
            // Project onto the nearest tangent from the destination to the
            // exclusion sphere. Preserve the axial position of distant jumps;
            // relocating to the body's centre plane creates enormous detours.
            let to_destination = destination.relative_to(blocker.pose.position);
            let distance = to_destination.length();
            let axis = to_destination / distance;
            let along = relative.dot(axis);
            let lateral = relative - axis * along;
            let side = lateral
                .try_normalize()
                .unwrap_or_else(|| direction.any_orthonormal_vector());
            let ignored = departure_body == Some(&blocker.reference);
            let radius = if ignored {
                blocker.radius_m
            } else {
                blocker.slip_exclusion_m.max(blocker.radius_m)
            } + own_radius
                + margin;
            if distance <= radius {
                return None;
            }
            let ratio = radius / distance;
            let cosine = (1. - ratio * ratio).sqrt();
            let slope = ratio / cosine;
            let required = radius * (1. - along / distance) / cosine;
            let step = (required - lateral.length()).max(0.) / (1. + slope * slope);
            candidate = candidate.offset_by((side + axis * slope) * step);
            continue;
        };

        velocity = obstacle.pose.velocity;
        let direction = candidate
            .relative_to(obstacle.pose.position)
            .try_normalize()
            .or_else(|| preferred_direction.try_normalize())
            .unwrap_or(DVec3::Z);
        let radius = obstacle.slip_exclusion_m + own_radius + margin;
        candidate = obstacle.pose.position.offset_by(direction * radius);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::{Id, travel::Destination};

    fn pose(position: DVec3) -> Pose {
        Pose {
            position: GalacticPosition::from_meters(position),
            velocity: [0.; 3],
            rotation: DQuat::IDENTITY.to_array(),
            angular_velocity: [0.; 3],
        }
    }

    fn station() -> LocalObstacle {
        LocalObstacle {
            reference: Target::Destination(Destination::Beacon(Id::new())),
            pose: pose(DVec3::ZERO),
            radius_m: 350.,
            slip_exclusion_m: 10_000_000.,
            hill_radius_m: 0.,
        }
    }

    #[test]
    fn departure_hill_sphere_allows_crossing_exclusion_but_requires_clear_origin() {
        let mut body = station();
        body.hill_radius_m = 1e10;
        let radius = body.slip_exclusion_m;
        let space = LocalSpace {
            obstacles: vec![body],
            truncated: false,
        };
        let origin = GalacticPosition::from_meters(DVec3::new(-2. * radius, radius * 0.5, 0.));
        let result = outside_exclusions(origin, DVec3::X * 1e15, &space, 10.).unwrap();
        assert_eq!(result.position, origin);
        let inside = GalacticPosition::from_meters(DVec3::X * radius * 0.9);
        let result = outside_exclusions(inside, DVec3::X * 1e15, &space, 10.).unwrap();
        assert!(result.position.relative_to(GalacticPosition::ZERO).length() > radius + 10.);
        let collision = GalacticPosition::from_meters(-DVec3::X * radius * 2.);
        let result = outside_exclusions(collision, DVec3::X * 1e15, &space, 10.).unwrap();
        assert!(result.position.relative_to(collision).length() > 350.);
        assert!(result.position.relative_to(collision).length() < 1000.);
    }

    #[test]
    fn departure_takes_clear_segments_around_the_station_to_the_opposite_destination() {
        let space = LocalSpace {
            obstacles: vec![station()],
            truncated: false,
        };
        let mut current = pose(DVec3::NEG_Z * 400.);
        let destination = pose(DVec3::Z * 100_000.);
        let mut avoidance = Avoidance::default();
        let mut reached = false;

        for _ in 0..16 {
            let steering = avoidance.steer(&current, &destination, &space, 10., None);
            assert!(
                intersection(
                    current.position,
                    steering.target.position,
                    GalacticPosition::ZERO,
                    360.,
                )
                .is_none()
            );
            current = steering.target;
            if !steering.detouring {
                reached = true;
                break;
            }
        }
        assert!(reached);
        assert_eq!(current.position, destination.position);
    }

    #[test]
    fn local_waypoint_moves_with_observed_station_and_keeps_the_same_side() {
        let mut space = LocalSpace {
            obstacles: vec![station()],
            truncated: false,
        };
        let mut current = pose(DVec3::NEG_Z * 400.);
        let destination = pose(DVec3::Z * 100_000.);
        let mut avoidance = Avoidance::default();
        let first = avoidance.steer(&current, &destination, &space, 10., None);
        let displacement = DVec3::new(100., 30., -50.);
        current.position = current.position.offset_by(displacement);
        space.obstacles[0].pose.position = space.obstacles[0].pose.position.offset_by(displacement);
        space.obstacles[0].pose.velocity = [1000., 300., -500.];
        let second = avoidance.steer(&current, &destination, &space, 10., None);
        assert!(!second.changed);
        assert!(
            second
                .target
                .position
                .relative_to(first.target.position)
                .distance(displacement)
                < 1e-6
        );
        assert_eq!(second.target.velocity, space.obstacles[0].pose.velocity);
    }

    #[test]
    fn incomplete_observations_brake_in_the_observed_local_frame() {
        let mut obstacle = station();
        obstacle.pose.velocity = [30_000., 0., 0.];
        let space = LocalSpace {
            obstacles: vec![obstacle],
            truncated: true,
        };
        let mut current = pose(DVec3::NEG_Z * 400.);
        current.velocity = [30_000., 0., 100.];
        let destination = pose(DVec3::Z * 100_000.);
        let steering = Avoidance::default().steer(&current, &destination, &space, 10., None);
        assert_eq!(steering.target.position, current.position);
        assert_eq!(steering.target.velocity, [30_000., 0., 0.]);
        assert!(steering.detouring);
    }

    #[test]
    fn another_observed_hull_cannot_be_ignored_by_a_remembered_detour() {
        let mut space = LocalSpace {
            obstacles: vec![station()],
            truncated: false,
        };
        let current = pose(DVec3::NEG_Z * 400.);
        let destination = pose(DVec3::Z * 100_000.);
        let mut avoidance = Avoidance::default();
        let first = avoidance.steer(&current, &destination, &space, 10., None);
        let mut blocker = station();
        blocker.pose.position = first.target.position;
        blocker.radius_m = 20.;
        blocker.slip_exclusion_m = 0.;
        space.obstacles.push(blocker);

        let next = avoidance.steer(&current, &destination, &space, 10., None);
        assert!(
            intersection(
                current.position,
                next.target.position,
                first.target.position,
                30.,
            )
            .is_none()
        );
    }

    #[test]
    fn departure_clears_overlapping_exclusions() {
        let mut other = station();
        other.pose.position = GalacticPosition::from_meters(DVec3::X * 5_000_000.);
        let space = LocalSpace {
            obstacles: vec![station(), other],
            truncated: false,
        };
        let end = outside_exclusions(GalacticPosition::ZERO, DVec3::NEG_X, &space, 10.).unwrap();
        for obstacle in &space.obstacles {
            assert!(
                end.position.relative_to(obstacle.pose.position).length()
                    > obstacle.slip_exclusion_m + 10.
            );
        }
        assert!(end.position.relative_to(GalacticPosition::ZERO).x < 0.);
    }

    #[test]
    fn departure_moves_around_exclusion_before_aiming_through_its_far_side() {
        let obstacle = station();
        let radius = obstacle.slip_exclusion_m;
        let space = LocalSpace {
            obstacles: vec![obstacle],
            truncated: false,
        };
        let start = GalacticPosition::from_meters(DVec3::NEG_X * radius * 1.01);
        let destination = GalacticPosition::from_meters(DVec3::X * radius * 100.);
        let departure =
            outside_exclusions(start, destination.relative_to(start), &space, 10.).unwrap();
        assert!(departure.position.relative_to(start).length() > radius);
        assert!(
            intersection(
                departure.position,
                destination,
                GalacticPosition::ZERO,
                radius + 10.
            )
            .is_none()
        );
    }

    #[test]
    fn stellar_surface_departure_needs_only_hull_clearance() {
        let mut star = station();
        star.slip_exclusion_m = 30e9;
        star.pose.velocity = [30_000., -2_000., 0.];
        let space = LocalSpace {
            obstacles: vec![star],
            truncated: false,
        };
        let start = GalacticPosition::from_meters(DVec3::X * (30e9 - 1.));
        let target = outside_exclusions(start, DVec3::X * 1e18, &space, 10.).unwrap();
        let travel = target.position.relative_to(start).length();
        assert!(
            (20. ..30.).contains(&travel),
            "unnecessary surface departure: {travel} m"
        );
        assert_eq!(target.velocity, space.obstacles[0].pose.velocity);
    }

    #[test]
    fn grazing_stellar_shadow_uses_nearest_clear_line() {
        let mut star = station();
        star.slip_exclusion_m = 10e9;
        let space = LocalSpace {
            obstacles: vec![star],
            truncated: false,
        };
        let start = GalacticPosition::from_meters(DVec3::new(-4e9, 10e9 - 1e6, 0.));
        let destination = GalacticPosition::from_meters(DVec3::X * 1e18);
        let target =
            outside_exclusions(start, destination.relative_to(start), &space, 10.).unwrap();
        assert!(target.position.relative_to(start).length() < 1.01e6);
        assert!(
            intersection(
                target.position,
                destination,
                GalacticPosition::ZERO,
                10e9 + 10.
            )
            .is_none()
        );
    }
}
