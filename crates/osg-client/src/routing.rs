//! Client-owned strategic search. Inputs are catalogue data and a frozen ship snapshot.
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, ensure};
use osg_model::{
    GalacticPosition, Id,
    travel::{self, Directive, ItineraryEntry, PlanningPreferences, slip},
};
use osg_universe::universe::Universe;
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Request {
    pub universe: Arc<Universe>,
    pub origin: GalacticPosition,
    pub current_system: Option<Id>,
    pub mass_kg: f64,
    pub exotic_kg: f64,
    pub preferences: PlanningPreferences,
    pub directives: Vec<Directive>,
    pub assisted: BTreeSet<Id>,
    pub stations: BTreeMap<Id, Id>,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub itinerary: Vec<ItineraryEntry>,
    pub seconds: f64,
    pub exotic_fuel_kg: f64,
    pub estimated_loss_ppm: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Searching,
    Optimal,
    Cancelled,
    Exhausted(String),
    Failed(String),
}

#[derive(Clone, Default)]
pub struct Progress {
    pub status: Status,
    pub elapsed: Duration,
    pub expanded: usize,
    pub queued: usize,
    /// Bit 0: explored; bit 1: queued. Several labels may share a system.
    pub systems: Vec<u8>,
    pub explored_systems: usize,
    pub samples: Vec<usize>,
    pub current: Option<usize>,
    pub branch: Vec<usize>,
    pub best: Option<Plan>,
}

struct Label {
    node: usize,
    stage: usize,
    parent: Option<usize>,
    seconds: f64,
    fuel: f64,
    loss: f64,
    depth: usize,
    active: bool,
    queued: bool,
}

struct Pending(f64, usize);
impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
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
            .0
            .total_cmp(&self.0)
            .then_with(|| other.1.cmp(&self.1))
    }
}

struct Search {
    request: Request,
    goals: Vec<usize>,
    /// Distance from each goal to its nearest accessible beacon. Every route
    /// must cover the final stretch after its last beacon at unassisted speed.
    unassisted_tail_m: Vec<f64>,
    states: Vec<Label>,
    labels: HashMap<(usize, usize), Vec<usize>>,
    frontier: BinaryHeap<Pending>,
    progress: Progress,
    started: Instant,
    checked_arrivals: bool,
}

impl Search {
    fn new(request: Request) -> Result<Self> {
        ensure!(request.preferences.valid(), "Invalid route preferences");
        ensure!(
            request.mass_kg.is_finite()
                && request.mass_kg > 0.
                && request.exotic_kg.is_finite()
                && request.exotic_kg >= 0.,
            "Ship resources unavailable"
        );
        ensure!(
            request.directives.len() <= travel::MAX_DIRECTIVES,
            "Too many directives"
        );
        let goals = request
            .directives
            .iter()
            .map(|directive| {
                let id = match directive {
                    Directive::SlipToSystem(id) => id,
                    Directive::DockAt(id) => request
                        .stations
                        .get(id)
                        .context("Docking destination unavailable")?,
                };
                request
                    .universe
                    .system_index(id.0)
                    .context("Unknown destination system")
            })
            .collect::<Result<Vec<_>>>()?;
        let count = request.universe.systems().len();
        let unassisted_tail_m = goals
            .iter()
            .map(|&goal| {
                let target = request.universe.systems()[goal].position;
                request
                    .assisted
                    .iter()
                    .filter_map(|id| request.universe.system_index(id.0))
                    .map(|index| {
                        request.universe.systems()[index]
                            .position
                            .relative_to(target)
                            .length()
                    })
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let mut search = Self {
            request,
            goals,
            unassisted_tail_m,
            states: Vec::new(),
            labels: HashMap::new(),
            frontier: BinaryHeap::new(),
            progress: Progress {
                systems: vec![0; count],
                ..Default::default()
            },
            started: Instant::now(),
            checked_arrivals: false,
        };
        let node = search
            .request
            .current_system
            .and_then(|id| search.request.universe.system_index(id.0));
        let (stage, docks) = search.advance_stage(node, 0);
        search.states.push(Label {
            node: count,
            stage,
            parent: None,
            seconds: 0.,
            fuel: 0.,
            loss: 0.,
            depth: docks,
            active: true,
            queued: true,
        });
        search
            .frontier
            .push(Pending(search.heuristic(search.request.origin, stage), 0));
        Ok(search)
    }

    fn advance_stage(&self, node: Option<usize>, mut stage: usize) -> (usize, usize) {
        let mut docks = 0;
        while self
            .goals
            .get(stage)
            .copied()
            .is_some_and(|goal| Some(goal) == node)
        {
            docks += usize::from(matches!(
                self.request.directives[stage],
                Directive::DockAt(_)
            ));
            stage += 1;
        }
        (stage, docks)
    }

    fn position(&self, node: usize) -> GalacticPosition {
        self.request
            .universe
            .systems()
            .get(node)
            .map_or(self.request.origin, |system| system.position)
    }

    fn heuristic(&self, mut origin: GalacticPosition, stage: usize) -> f64 {
        let mut seconds = 0.;
        for (&goal, &tail) in self.goals[stage..]
            .iter()
            .zip(&self.unassisted_tail_m[stage..])
        {
            let target = self.position(goal);
            let distance = target.relative_to(origin).length();
            if distance > 0. {
                // Total distance is at least the straight-line distance. The
                // final unassisted portion starts either here or at a beacon.
                // This remains a lower bound even with arbitrary intermediate
                // stars, and reduces to unassisted flight when no beacon exists.
                let fast = slip::cruise_speed_ly_s(true) * slip::LY_M;
                let slow = slip::cruise_speed_ly_s(false) * slip::LY_M;
                let flight = distance / fast + distance.min(tail) * (1. / slow - 1. / fast);
                seconds += slip::MIN_CHARGE_SECONDS + flight.max(slip::MIN_TRANSIT_SECONDS);
            }
            origin = target;
        }
        seconds
    }

    fn path(&self, mut index: usize) -> Vec<usize> {
        let mut path = vec![index];
        while let Some(parent) = self.states[index].parent {
            path.push(parent);
            index = parent;
        }
        path.reverse();
        path
    }

    fn plan(&self, index: usize) -> Plan {
        let mut itinerary = Vec::new();
        let mut stage = 0;
        for index in self.path(index) {
            let state = &self.states[index];
            if state.parent.is_some() {
                let system = &self.request.universe.systems()[state.node];
                itinerary.push(ItineraryEntry {
                    directive: Directive::SlipToSystem(Id(system.id)),
                    label: format!("Slip · {}", system.name),
                });
            }
            for directive in &self.request.directives[stage..state.stage] {
                if matches!(directive, Directive::DockAt(_)) {
                    itinerary.push(ItineraryEntry {
                        directive: directive.clone(),
                        label: directive.label(),
                    });
                }
            }
            stage = state.stage;
        }
        let state = &self.states[index];
        Plan {
            itinerary,
            seconds: state.seconds,
            exotic_fuel_kg: state.fuel,
            estimated_loss_ppm: slip::ppm_from_log_loss(state.loss),
        }
    }

    fn check_arrivals(&mut self, proceed: &impl Fn() -> bool) -> bool {
        let fuel_radius = slip::exotic_range_ly(
            self.request.mass_kg,
            self.request.exotic_kg * self.request.preferences.fuel_fraction,
        ) * slip::LY_M;
        // Reverse addition can round differently from forward itinerary sums.
        // Keep this rejection bound conservative for every permitted hop count.
        let rounding = 4. * travel::MAX_DIRECTIVES as f64 * f64::EPSILON;
        let limit = (slip::log_loss_from_ppm(self.request.preferences.max_loss_ppm)
            * (1. + rounding))
            .next_up();
        if !limit.is_finite() {
            return true;
        }

        // Walk backwards using minimum cumulative loss. Treat reaching any
        // accessible beacon as success, and relax the total fuel constraint to
        // a per-leg bound. This can prove a destination disconnected, but never
        // rejects a feasible route. The forward search still checks all budgets.
        for &goal in &self.goals {
            let target = &self.request.universe.systems()[goal];
            if self.request.current_system == Some(Id(target.id)) {
                continue;
            }
            let mut pending = BinaryHeap::from([Pending(0., goal)]);
            let mut losses = HashMap::from([(goal, 0.)]);
            let mut reachable = false;
            while let Some(Pending(loss, node)) = pending.pop() {
                if !proceed() {
                    break;
                }
                if loss > losses[&node] {
                    continue;
                }
                let arrival = &self.request.universe.systems()[node];
                if self.request.assisted.contains(&Id(arrival.id)) {
                    reachable = true;
                    break;
                }
                let exclusion = slip::exclusion_radius_m(arrival.stellar_mass);
                let arrival_loss = |distance| {
                    loss + slip::log_loss_from_ppm(slip::capture_loss_ppm(
                        exclusion, distance, false,
                    ))
                };
                let departure_distance = arrival.position.relative_to(self.request.origin).length();
                if departure_distance <= fuel_radius * (1. + 1e-12)
                    && arrival_loss(departure_distance) <= limit
                {
                    reachable = true;
                    break;
                }

                // Include underflow-to-zero probabilities accepted by the edge
                // model, and round the candidate radius outwards.
                let probability =
                    (slip::ppm_from_log_loss(limit - loss) / 1e6).max(f64::from_bits(1));
                let risk_radius =
                    exclusion / (slip::dispersion_rad(false) * (-2. * probability.ln()).sqrt());
                let radius = fuel_radius.min(risk_radius) * (1. + 1e-12);
                self.request
                    .universe
                    .index()
                    .spatial
                    .visit_centers_within_radius(arrival.position, radius, proceed, |next| {
                        if next == node {
                            return;
                        }
                        let position = self.request.universe.systems()[next].position;
                        let next_loss =
                            arrival_loss(position.relative_to(arrival.position).length());
                        if next_loss <= limit
                            && losses.get(&next).is_none_or(|&old| next_loss < old)
                        {
                            losses.insert(next, next_loss);
                            pending.push(Pending(next_loss, next));
                        }
                    });
            }
            if !proceed() {
                self.progress.status = Status::Cancelled;
                return false;
            }
            if !reachable {
                self.progress.status = Status::Exhausted(format!(
                    "No chain of arrivals to {} fits the trip risk limit with current beacon access and fuel allowance.",
                    target.name
                ));
                return false;
            }
        }
        true
    }

    fn expand(
        &mut self,
        proceed: &impl Fn() -> bool,
        publish: &mut impl FnMut(&mut Self),
    ) -> Result<bool> {
        if !self.checked_arrivals {
            self.checked_arrivals = true;
            if !self.check_arrivals(proceed) {
                return Ok(false);
            }
        }
        let Some(Pending(bound, index)) = self.frontier.pop() else {
            self.progress.status = if self.progress.best.is_some() {
                Status::Optimal
            } else {
                Status::Exhausted(
                    "No feasible route within trip limits and itinerary capacity".into(),
                )
            };
            return Ok(false);
        };
        if !self.states[index].active {
            return Ok(true);
        }
        if self
            .progress
            .best
            .as_ref()
            .is_some_and(|best| bound >= best.seconds)
        {
            self.progress.status = Status::Optimal;
            return Ok(false);
        }
        let state = &mut self.states[index];
        state.queued = false;
        self.progress.expanded += 1;
        let (node, stage, elapsed, used, loss, depth) = (
            state.node,
            state.stage,
            state.seconds,
            state.fuel,
            state.loss,
            state.depth,
        );
        if stage == self.goals.len() {
            self.progress.best = Some(self.plan(index));
            return Ok(true);
        }
        if !self.request.preferences.allow_slipdrive || depth >= travel::MAX_DIRECTIVES {
            return Ok(true);
        }
        let origin = self.position(node);
        let display_node = |node| {
            if node < self.progress.systems.len() {
                Some(node)
            } else {
                self.request
                    .current_system
                    .and_then(|id| self.request.universe.system_index(id.0))
            }
        };
        self.progress.current = display_node(node);
        self.progress.branch = self
            .path(index)
            .into_iter()
            .filter_map(|index| display_node(self.states[index].node))
            .collect();
        let fuel_limit = self.request.exotic_kg * self.request.preferences.fuel_fraction;
        let radius =
            slip::exotic_range_ly(self.request.mass_kg, (fuel_limit - used).max(0.)) * slip::LY_M;
        let universe = self.request.universe.clone();
        let mut failure = None;
        let completed =
            universe
                .index()
                .spatial
                .visit_centers_within_radius(origin, radius, proceed, |next| {
                    publish(self);
                    if next == node || failure.is_some() {
                        return;
                    }
                    let target = &universe.systems()[next];
                    if index == 0 && self.request.current_system == Some(Id(target.id)) {
                        return;
                    }
                    let assisted = self.request.assisted.contains(&Id(target.id));
                    let distance = target.position.relative_to(origin).length();
                    let fuel =
                        used + slip::exotic_fuel_kg(self.request.mass_kg, distance / slip::LY_M);
                    let risk = loss
                        + slip::log_loss_from_ppm(slip::capture_loss_ppm(
                            slip::exclusion_radius_m(target.stellar_mass),
                            distance,
                            assisted,
                        ));
                    if fuel > fuel_limit
                        || !risk.is_finite()
                        || risk > slip::log_loss_from_ppm(self.request.preferences.max_loss_ppm)
                    {
                        return;
                    }
                    let seconds = elapsed
                        + slip::MIN_CHARGE_SECONDS
                        + slip::flight_seconds(distance, assisted);
                    let (next_stage, docks) = self.advance_stage(Some(next), stage);
                    let next_depth = depth + 1 + docks;
                    if next_depth > travel::MAX_DIRECTIVES {
                        return;
                    }
                    let estimate = seconds + self.heuristic(target.position, next_stage);
                    if self
                        .progress
                        .best
                        .as_ref()
                        .is_some_and(|best| estimate >= best.seconds)
                    {
                        return;
                    }
                    let labels = self.labels.entry((next, next_stage)).or_default();
                    if labels.iter().any(|&i| {
                        let old = &self.states[i];
                        old.seconds <= seconds
                            && old.fuel <= fuel
                            && old.loss <= risk
                            && old.depth <= next_depth
                    }) {
                        return;
                    }
                    labels.retain(|&i| {
                        let old = &mut self.states[i];
                        if seconds <= old.seconds
                            && fuel <= old.fuel
                            && risk <= old.loss
                            && next_depth <= old.depth
                        {
                            old.active = false;
                            false
                        } else {
                            true
                        }
                    });
                    if let Err(error) = self.states.try_reserve(1) {
                        failure = Some(anyhow::anyhow!("Search storage unavailable: {error}"));
                        return;
                    }
                    let id = self.states.len();
                    labels.push(id);
                    self.states.push(Label {
                        node: next,
                        stage: next_stage,
                        parent: Some(index),
                        seconds,
                        fuel,
                        loss: risk,
                        depth: next_depth,
                        active: true,
                        queued: true,
                    });
                    self.frontier.push(Pending(estimate, id));
                    if next_stage == self.goals.len() {
                        self.progress.best = Some(self.plan(id));
                    }
                });
        if let Some(error) = failure {
            return Err(error);
        }
        if !completed {
            self.progress.status = Status::Cancelled;
        }
        Ok(completed)
    }

    fn snapshot(&mut self) -> Arc<Progress> {
        self.progress.elapsed = self.started.elapsed();
        self.progress.systems.fill(0);
        self.progress.queued = 0;
        for label in &self.states {
            if let Some(flag) = self.progress.systems.get_mut(label.node) {
                if !label.queued {
                    *flag |= 1;
                }
                if label.active && label.queued {
                    *flag |= 2;
                    self.progress.queued += 1;
                }
            }
        }
        self.progress.explored_systems = self
            .progress
            .systems
            .iter()
            .filter(|&&state| state & 1 != 0)
            .count();
        let displayed = self
            .progress
            .systems
            .iter()
            .filter(|&&state| state != 0)
            .count();
        let stride = displayed.div_ceil(8192).max(1);
        self.progress.samples = self
            .progress
            .systems
            .iter()
            .enumerate()
            .filter_map(|(index, &state)| (state != 0).then_some(index))
            .step_by(stride)
            .collect();
        Arc::new(self.progress.clone())
    }
}

struct Mailbox {
    request: Option<(u64, Request)>,
    progress: Arc<Progress>,
    shutdown: bool,
}

/// One persistent worker. Replacing a request cancels the previous generation.
pub struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    generation: Arc<AtomicU64>,
}

impl Default for Worker {
    fn default() -> Self {
        let shared = Arc::new((
            Mutex::new(Mailbox {
                request: None,
                progress: Arc::new(Progress::default()),
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let generation = Arc::new(AtomicU64::new(0));
        let (state, current) = (shared.clone(), generation.clone());
        std::thread::spawn(move || {
            loop {
                let (lock, changed) = &*state;
                let mut mailbox = lock.lock().unwrap();
                while mailbox.request.is_none() && !mailbox.shutdown {
                    mailbox = changed.wait(mailbox).unwrap();
                }
                if mailbox.shutdown {
                    return;
                }
                let (id, request) = mailbox.request.take().unwrap();
                drop(mailbox);
                let proceed = || current.load(Ordering::Relaxed) == id;
                let publish = |progress| {
                    let mut mailbox = lock.lock().unwrap();
                    if proceed() {
                        mailbox.progress = progress;
                    }
                };
                let mut search = match Search::new(request) {
                    Ok(search) => search,
                    Err(error) => {
                        publish(Arc::new(Progress {
                            status: Status::Failed(error.to_string()),
                            ..Default::default()
                        }));
                        continue;
                    }
                };
                let mut published = Instant::now();
                let mut had_incumbent = false;
                while proceed() {
                    let running = match search.expand(&proceed, &mut |search| {
                        let first_incumbent = !had_incumbent && search.progress.best.is_some();
                        if first_incumbent || published.elapsed() >= Duration::from_millis(100) {
                            had_incumbent |= first_incumbent;
                            publish(search.snapshot());
                            published = Instant::now();
                        }
                    }) {
                        Ok(running) => running,
                        Err(error) => {
                            search.progress.status = Status::Failed(error.to_string());
                            false
                        }
                    };
                    let first_incumbent = !had_incumbent && search.progress.best.is_some();
                    had_incumbent |= first_incumbent;
                    if !running
                        || first_incumbent
                        || published.elapsed() >= Duration::from_millis(100)
                    {
                        publish(search.snapshot());
                        published = Instant::now();
                    }
                    if !running {
                        break;
                    }
                }
            }
        });
        Self { shared, generation }
    }
}

impl Worker {
    pub fn start(&self, request: Request) {
        let mut mailbox = self.shared.0.lock().unwrap();
        let id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        mailbox.progress = Arc::new(Progress::default());
        mailbox.request = Some((id, request));
        self.shared.1.notify_one();
    }

    pub fn progress(&self) -> Arc<Progress> {
        self.shared.0.lock().unwrap().progress.clone()
    }

    pub fn cancel(&self) {
        let mut mailbox = self.shared.0.lock().unwrap();
        self.generation.fetch_add(1, Ordering::Relaxed);
        mailbox.request = None;
        let progress = Arc::make_mut(&mut mailbox.progress);
        if progress.status == Status::Searching {
            progress.status = Status::Cancelled;
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.shared.0.lock().unwrap().shutdown = true;
        self.shared.1.notify_one();
    }
}
