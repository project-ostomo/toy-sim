use super::*;
use rayon::prelude::*;

const RELATIVE_SPEED_LIMIT: f64 = 1_000_000.0;

#[cfg(test)]
pub(super) fn advance(
    bodies: &mut Vec<Body>,
    dt: f64,
    spatial: &mut GalacticIndex<SpatialKey>,
    workspace: &mut SolverWorkspace,
    allocate: &mut dyn FnMut() -> Entity,
) -> Report {
    let mut report = Report::default();
    synchronize(bodies, spatial);
    activate(bodies, spatial);
    fire(bodies, dt, workspace, allocate, &mut report);
    synchronize(bodies, spatial);
    for beam in std::mem::take(&mut report.beams) {
        weapons::resolve_beam(beam, bodies, spatial, 0.0, &mut report);
    }
    let impacts = integrate(bodies, dt, spatial, workspace, &mut report);
    damage(bodies, dt, impacts, &mut report);
    report
}

pub(super) fn synchronize(bodies: &[Body], spatial: &mut GalacticIndex<SpatialKey>) {
    for body in bodies {
        let key = SpatialKey::Entity(body.entity);
        let luminosity = spatial.get(&key).map_or(0.0, |record| record.luminosity);
        spatial
            .insert(
                key,
                SpatialRecord {
                    position: body.position,
                    radius_m: body.radius,
                    luminosity,
                },
            )
            .expect("collision body coordinate range");
    }
}

pub(super) fn fire(
    bodies: &mut Vec<Body>,
    dt: f64,
    workspace: &mut SolverWorkspace,
    allocate: &mut dyn FnMut() -> Entity,
    report: &mut Report,
) {
    let mut projectiles = Vec::new();
    for body in bodies.iter_mut() {
        record_deaths(body, 0.0, report);
        if !body.alive() {
            continue;
        }
        for member in 0..body.members.len() {
            if body.members[member].destroyed {
                continue;
            }
            let Some(ship) = workspace.weapons.get_mut(&body.members[member].entity) else {
                continue;
            };
            for weapon in 0..ship.weapons.len() {
                let mut after = 0.0;
                while let Some(at) = weapons::next_event(weapon, ship, workspace.time_s, after, dt)
                {
                    if at >= dt {
                        break;
                    }
                    let previous = ship.weapons[weapon].shots_fired;
                    // Cadence uses due times; every shot's physical launch is at
                    // this boundary. No body integration occurs while batching.
                    if let Some(projectile) = weapons::fire(
                        body,
                        member,
                        weapon,
                        ship,
                        workspace.time_s + at,
                        0.0,
                        allocate,
                        report,
                    ) {
                        projectiles.push(projectile);
                    }
                    if ship.weapons[weapon].shots_fired == previous {
                        break;
                    }
                    after = at + 1e-9;
                }
            }
            weapons::advance(ship, body, member, workspace.time_s, dt);
        }
    }
    bodies.extend(projectiles);
}

fn root(parents: &mut [usize], mut id: usize) -> usize {
    while parents[id] != id {
        parents[id] = parents[parents[id]];
        id = parents[id];
    }
    id
}

fn physical_input(body: &Body) -> rapier::Input {
    let parts: Vec<_> = body
        .members
        .iter()
        .filter(|member| !member.destroyed)
        .map(|member| {
            let shape = if member.shielded() {
                SharedShape::ball(member.geometry.shield_radius)
            } else {
                member.geometry.surface.clone()
            };
            (pose(member.local_position, member.local_rotation), shape)
        })
        .collect();
    let shape = if parts.len() == 1 && parts[0].0 == Pose::IDENTITY {
        parts[0].1.clone()
    } else {
        SharedShape::compound(parts)
    };
    rapier::Input {
        entity: body.entity,
        position: body.position,
        rotation: body.rotation,
        velocity: body.velocity,
        angular: body.world_inverse() * body.momentum,
        mass: body.mass,
        inertia: body.inertia_inv.inverse(),
        shape,
        launch_owner: body.launch_owner,
    }
}

pub(super) fn integrate(
    bodies: &mut [Body],
    dt: f64,
    spatial: &GalacticIndex<SpatialKey>,
    workspace: &mut SolverWorkspace,
    report: &mut Report,
) -> Vec<rapier::Impact> {
    let started = std::time::Instant::now();
    let slots: HashMap<_, _> = bodies
        .iter()
        .enumerate()
        .map(|(id, body)| (body.entity, id))
        .collect();
    let maximum_radius = bodies.iter().map(|body| body.radius).fold(0.0, f64::max);
    let mut parents: Vec<_> = (0..bodies.len()).collect();
    let mut participates = vec![false; bodies.len()];
    let mut speed_violations = 0;
    for (a, body) in bodies.iter().enumerate().filter(|(_, body)| body.alive()) {
        let range = RELATIVE_SPEED_LIMIT * dt + body.radius + maximum_radius;
        for key in spatial
            .within_radius(body.position, range, false)
            .expect("group query coordinates")
        {
            let SpatialKey::Entity(entity) = key else {
                continue;
            };
            let Some(&b) = slots.get(&entity) else {
                continue;
            };
            if a >= b || !bodies[b].alive() {
                continue;
            }
            let bound = RELATIVE_SPEED_LIMIT * dt + body.radius + bodies[b].radius;
            if bodies[b].position.relative_to(body.position).length() > bound {
                continue;
            }
            if (bodies[b].velocity - body.velocity).length() > RELATIVE_SPEED_LIMIT {
                speed_violations += 1;
            }
            report.candidates += 1;
            participates[a] = true;
            participates[b] = true;
            let left = root(&mut parents, a);
            let right = root(&mut parents, b);
            parents[right] = left;
        }
    }
    if speed_violations > 0 {
        bevy::log::warn!(
            speed_violations,
            "collision relative-speed assumption exceeded"
        );
    }
    report.query_seconds = started.elapsed().as_secs_f64();

    let started = std::time::Instant::now();
    let mut groups: HashMap<usize, Vec<rapier::Input>> = HashMap::new();
    for (id, body) in bodies
        .iter_mut()
        .enumerate()
        .filter(|(_, body)| body.alive())
    {
        record_motion(body, dt, report);
        if participates[id] {
            groups
                .entry(root(&mut parents, id))
                .or_default()
                .push(physical_input(body));
        } else {
            body.rebase(dt);
        }
    }
    let jobs: Vec<_> = groups
        .into_values()
        .map(|input| {
            let mut key: Vec<_> = input.iter().map(|body| body.entity).collect();
            key.sort_unstable();
            let cached = workspace.worlds.remove(&key);
            report.reused_worlds += usize::from(cached.is_some());
            let world = cached.unwrap_or_default();
            (key, world, input)
        })
        .collect();
    report.groups = jobs.len();
    report.grouped_bodies = participates.iter().filter(|&&value| value).count();
    let previous = &workspace.contacts;
    let completed: Vec<_> = jobs
        .into_par_iter()
        .map(|(key, mut world, input)| {
            let output = world.step(&input, dt, previous);
            (key, world, output)
        })
        .collect();
    workspace.worlds.clear();
    workspace.contacts.clear();
    let mut impacts = Vec::new();
    for (key, world, output) in completed {
        report.contact_pairs += output.contact_pairs;
        workspace.worlds.insert(key, world);
        workspace.contacts.extend(output.contacts);
        impacts.extend(output.impacts);
        for motion in output.motion {
            let body = &mut bodies[slots[&motion.entity]];
            body.impulse_dv += motion.velocity - body.velocity;
            body.position = motion.position;
            body.rotation = motion.rotation;
            body.velocity = motion.velocity;
            body.momentum = motion.rotation
                * (body.inertia_inv.inverse() * (motion.rotation.inverse() * motion.angular));
            body.time = dt;
        }
    }
    // Tracers end at Rapier's actual pose, including CCD clamping. This is the
    // boundary-to-boundary path; no speculative ballistic segment crosses a hull.
    for segment in &mut report.motion {
        let body = &bodies[slots[&segment.entity]];
        segment.velocity =
            body.position.relative_to(segment.position) / (segment.end - segment.start);
    }
    report.solve_seconds = started.elapsed().as_secs_f64();
    impacts
}

pub(super) fn damage(
    bodies: &mut [Body],
    dt: f64,
    impacts: Vec<rapier::Impact>,
    report: &mut Report,
) {
    let slots: HashMap<_, _> = bodies
        .iter()
        .enumerate()
        .map(|(id, body)| (body.entity, id))
        .collect();
    for impact in impacts {
        let mut shields = [false; 2];
        let mut velocity = DVec3::ZERO;
        for (side, entity) in impact.entities.into_iter().enumerate() {
            let body = &mut bodies[slots[&entity]];
            velocity += body.velocity * 0.5;
            // Current production bodies each own one hull. The nearest member
            // also handles compound assembly fixtures without moving ownership.
            let member = body
                .members
                .iter_mut()
                .filter(|member| !member.destroyed)
                .min_by(|a, b| {
                    let relative = impact.position.relative_to(body.position);
                    (body.rotation * a.local_position - relative)
                        .length_squared()
                        .total_cmp(&(body.rotation * b.local_position - relative).length_squared())
                });
            if let Some(member) = member {
                shields[side] = member.shielded();
                member.deposit(shields[side], impact.energy_j * 0.5);
            }
        }
        report.impact_events.push(ImpactEvent {
            entities: impact.entities,
            time: dt,
            position: impact.position,
            velocity,
            normal: impact.normal,
            shields,
            surface_positions: [impact.position; 2],
            energy_j: impact.energy_j,
        });
        report.impacts += 1;
        report.dissipated_j += impact.energy_j;
    }
    for body in bodies {
        body.advance_thermal(dt);
        if body.expires_at.is_some_and(|expires| expires <= dt) {
            for member in &mut body.members {
                member.hull = 0.0;
            }
        }
        record_deaths(body, dt, report);
    }
}
