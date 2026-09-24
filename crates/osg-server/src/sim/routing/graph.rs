use super::*;
use osg_model::travel::slip;
use std::collections::BinaryHeap;

struct State {
    node: usize,
    parent: Option<usize>,
    seconds: f64,
    fuel: f64,
    depth: usize,
}

struct Pending {
    estimate: f64,
    state: usize,
}

impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        self.estimate == other.estimate && self.state == other.state
    }
}
impl Eq for Pending {}
impl PartialOrd for Pending {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Pending {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then_with(|| other.state.cmp(&self.state))
    }
}

pub(super) fn search(
    request: &RouteRequest,
    environment: &impl RouteEnvironment,
    work: &mut Work,
    origin: GalacticPosition,
    goal: SystemTarget,
    fuel_limit: f64,
) -> Result<Vec<SystemTarget>> {
    let mut nodes = environment.candidates(origin, goal.position, 256)?;
    work.charge(ENVIRONMENT_WORK, environment)?;
    nodes.retain(|node| node.id != goal.id);
    nodes.sort_by_key(|node| node.id);
    nodes.dedup_by_key(|node| node.id);
    nodes.insert(0, goal);
    let source = nodes.len();
    nodes.push(SystemTarget {
        id: goal.id,
        position: origin,
        influence_m: 0.0,
    });
    let mut states = vec![State {
        node: source,
        parent: None,
        seconds: 0.0,
        fuel: 0.0,
        depth: 0,
    }];
    let mut frontier = BinaryHeap::from([Pending {
        estimate: 0.0,
        state: 0,
    }]);
    let mut labels = vec![Vec::<usize>::new(); nodes.len()];
    labels[source].push(0);

    while let Some(pending) = frontier.pop() {
        work.charge(1, environment)?;
        let current = &states[pending.state];
        if !labels[current.node].contains(&pending.state) {
            continue;
        }
        if current.node == 0 {
            let mut path = Vec::new();
            let mut index = pending.state;
            while let Some(parent) = states[index].parent {
                path.push(nodes[states[index].node]);
                index = parent;
            }
            path.reverse();
            return Ok(path);
        }
        if current.depth >= MAX_DIRECTIVES {
            continue;
        }
        let from = nodes[current.node].position;
        let elapsed = current.seconds;
        let used = current.fuel;
        let depth = current.depth;
        for next in 0..source {
            work.charge(1, environment)?;
            let distance = nodes[next].position.relative_to(from).length();
            if distance <= 0.0 {
                continue;
            }
            let fuel =
                used + slip::exotic_fuel_kg(request.performance.mass_kg, distance / slip::LY_M);
            if fuel > fuel_limit + 1e-9 {
                continue;
            }
            let seconds = elapsed + hop_seconds(&request.performance, distance);
            if labels[next]
                .iter()
                .any(|&index| states[index].seconds <= seconds && states[index].fuel <= fuel)
            {
                continue;
            }
            labels[next]
                .retain(|&index| !(seconds <= states[index].seconds && fuel <= states[index].fuel));
            let index = states.len();
            states.push(State {
                node: next,
                parent: Some(pending.state),
                seconds,
                fuel,
                depth: depth + 1,
            });
            labels[next].push(index);
            // Cruise time is a lower bound; charging and additional stops only add time.
            let remaining = goal.position.relative_to(nodes[next].position).length()
                / (slip::CRUISE_SPEED_LY_S * slip::LY_M);
            frontier.push(Pending {
                estimate: seconds + remaining,
                state: index,
            });
        }
    }
    anyhow::bail!("No system route fits the selected exotic fuel allowance")
}
