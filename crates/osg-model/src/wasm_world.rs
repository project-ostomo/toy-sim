use crate::{GalacticPosition, Id, ProgramAction, ProgramQuery, ProgramReply, travel};
use osg_ship_api::{abi::Text, world as abi};

#[derive(Clone, Copy, Debug, Default)]
pub struct ReplyCapacity {
    pub records: usize,
    pub auxiliary: usize,
    pub bytes: usize,
}

impl ReplyCapacity {
    pub const UNLIMITED: Self = Self {
        records: usize::MAX,
        auxiliary: usize::MAX,
        bytes: usize::MAX,
    };
}

fn flag(value: u64) -> Result<bool, ()> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(()),
    }
}

fn optional<T>(present: u64, value: T) -> Result<Option<T>, ()> {
    Ok(flag(present)?.then_some(value))
}

pub fn text<const N: usize>(value: &Text<N>) -> Result<String, ()> {
    let len = usize::try_from(value.len).map_err(|_| ())?;
    Ok(std::str::from_utf8(value.bytes.get(..len).ok_or(())?)
        .map_err(|_| ())?
        .to_owned())
}

pub fn position_record(value: &GalacticPosition) -> abi::Position {
    let mut words = [0; 6];
    for (index, coordinate) in [value.x, value.y, value.z].into_iter().enumerate() {
        words[index * 2] = coordinate as u64;
        words[index * 2 + 1] = (coordinate >> 64) as u64;
    }
    abi::Position { words }
}

pub fn position_value(value: &abi::Position) -> GalacticPosition {
    let coordinate = |index: usize| {
        ((value.words[index * 2 + 1] as i128) << 64) | value.words[index * 2] as i128
    };
    GalacticPosition::new(coordinate(0), coordinate(1), coordinate(2))
}

impl From<&crate::Pose> for abi::Pose {
    fn from(value: &crate::Pose) -> Self {
        Self {
            position: position_record(&value.position),
            velocity: value.velocity,
            rotation: value.rotation,
            angular_velocity: value.angular_velocity,
        }
    }
}

impl From<&abi::Pose> for crate::Pose {
    fn from(value: &abi::Pose) -> Self {
        Self {
            position: position_value(&value.position),
            velocity: value.velocity,
            rotation: value.rotation,
            angular_velocity: value.angular_velocity,
        }
    }
}

impl From<&crate::ContactRef> for abi::ContactRef {
    fn from(value: &crate::ContactRef) -> Self {
        Self {
            observer: value.observer.0,
            contact: value.contact,
        }
    }
}

impl From<&abi::ContactRef> for crate::ContactRef {
    fn from(value: &abi::ContactRef) -> Self {
        Self {
            observer: Id(value.observer),
            contact: value.contact,
        }
    }
}

impl From<&travel::Destination> for abi::Destination {
    fn from(value: &travel::Destination) -> Self {
        let mut output = Self::default();
        match value {
            travel::Destination::Beacon(id) => output.entity = id.0,
            travel::Destination::Galactic(position) => {
                output.kind = 1;
                output.position = position_record(position);
            }
            travel::Destination::Relative {
                reference,
                offset,
                axes,
            } => {
                let (kind, id) = match reference {
                    travel::Reference::Celestial(reference) => {
                        output.system = reference.system.0;
                        (2, &reference.body)
                    }
                    travel::Reference::Beacon(id) => (3, id),
                };
                output.kind = kind;
                output.entity = id.0;
                output.position = position_record(offset);
                output.axes = matches!(axes, travel::Axes::BodyFixed) as u64;
            }
        }
        output
    }
}

impl TryFrom<&abi::Destination> for travel::Destination {
    type Error = ();

    fn try_from(value: &abi::Destination) -> Result<Self, ()> {
        Ok(match value.kind {
            0 => Self::Beacon(Id(value.entity)),
            1 => Self::Galactic(position_value(&value.position)),
            2 | 3 => Self::Relative {
                reference: if value.kind == 2 {
                    travel::Reference::Celestial(travel::CelestialRef {
                        system: Id(value.system),
                        body: Id(value.entity),
                    })
                } else {
                    travel::Reference::Beacon(Id(value.entity))
                },
                offset: position_value(&value.position),
                axes: if flag(value.axes)? {
                    travel::Axes::BodyFixed
                } else {
                    travel::Axes::Galactic
                },
            },
            _ => return Err(()),
        })
    }
}

impl From<&travel::Target> for abi::Target {
    fn from(value: &travel::Target) -> Self {
        let mut output = Self::default();
        match value {
            travel::Target::Direction(direction) => output.direction = *direction,
            travel::Target::Destination(destination) => {
                output.kind = 1;
                output.destination = destination.into();
            }
            travel::Target::Contact(contact) => {
                output.kind = 2;
                output.contact = contact.into();
            }
        }
        output
    }
}

impl TryFrom<&abi::Target> for travel::Target {
    type Error = ();

    fn try_from(value: &abi::Target) -> Result<Self, ()> {
        Ok(match value.kind {
            0 => Self::Direction(value.direction),
            1 => Self::Destination((&value.destination).try_into()?),
            2 => Self::Contact((&value.contact).into()),
            _ => return Err(()),
        })
    }
}

impl From<&travel::Directive> for abi::Directive {
    fn from(value: &travel::Directive) -> Self {
        let (kind, entity) = match value {
            travel::Directive::SlipToSystem(id) => (abi::DIRECTIVE_SLIP_TO_SYSTEM, id.0),
            travel::Directive::DockAt(id) => (abi::DIRECTIVE_DOCK_AT, id.0),
        };
        Self { kind, entity }
    }
}

pub fn guidance_record(value: &Option<travel::Guidance>) -> abi::Guidance {
    let Some(value) = value else {
        return abi::Guidance::default();
    };
    abi::Guidance {
        present: 1,
        mode: match value.mode {
            travel::GuidanceMode::Align => abi::GUIDANCE_ALIGN,
            travel::GuidanceMode::Approach => abi::GUIDANCE_APPROACH,
            travel::GuidanceMode::KeepRange => abi::GUIDANCE_KEEP_RANGE,
        },
        target: (&value.target).into(),
        range_m: value.range_m,
    }
}

pub fn guidance_value(value: &abi::Guidance) -> Result<Option<travel::Guidance>, ()> {
    if !flag(value.present)? {
        return Ok(None);
    }
    if !value.range_m.is_finite() || value.range_m < 0.0 {
        return Err(());
    }
    Ok(Some(travel::Guidance {
        mode: match value.mode {
            abi::GUIDANCE_ALIGN => travel::GuidanceMode::Align,
            abi::GUIDANCE_APPROACH => travel::GuidanceMode::Approach,
            abi::GUIDANCE_KEEP_RANGE => travel::GuidanceMode::KeepRange,
            _ => return Err(()),
        },
        target: (&value.target).try_into()?,
        range_m: value.range_m,
    }))
}

impl TryFrom<&abi::Directive> for travel::Directive {
    type Error = ();

    fn try_from(value: &abi::Directive) -> Result<Self, ()> {
        match value.kind {
            abi::DIRECTIVE_SLIP_TO_SYSTEM => Ok(Self::SlipToSystem(Id(value.entity))),
            abi::DIRECTIVE_DOCK_AT => Ok(Self::DockAt(Id(value.entity))),
            _ => Err(()),
        }
    }
}

impl From<&travel::ItineraryEntry> for abi::ItineraryEntry {
    fn from(value: &travel::ItineraryEntry) -> Self {
        Self {
            label: Text::new(&value.label),
            directive: (&value.directive).into(),
        }
    }
}

impl TryFrom<&abi::ItineraryEntry> for travel::ItineraryEntry {
    type Error = ();

    fn try_from(value: &abi::ItineraryEntry) -> Result<Self, ()> {
        Ok(Self {
            label: text(&value.label)?,
            directive: (&value.directive).try_into()?,
        })
    }
}

impl From<&travel::PlanningPreferences> for abi::Preferences {
    fn from(value: &travel::PlanningPreferences) -> Self {
        Self {
            fuel_fraction: value.fuel_fraction,
            max_loss_ppm: value.max_loss_ppm,
            allow_slipdrive: value.allow_slipdrive as u64,
        }
    }
}

impl TryFrom<&abi::Preferences> for travel::PlanningPreferences {
    type Error = ();

    fn try_from(value: &abi::Preferences) -> Result<Self, ()> {
        let preferences = Self {
            fuel_fraction: value.fuel_fraction,
            max_loss_ppm: value.max_loss_ppm,
            allow_slipdrive: flag(value.allow_slipdrive)?,
        };
        preferences.valid().then_some(preferences).ok_or(())
    }
}

impl From<&crate::LocalObstacle> for abi::LocalObstacle {
    fn from(value: &crate::LocalObstacle) -> Self {
        Self {
            reference: (&value.reference).into(),
            pose: (&value.pose).into(),
            radius_m: value.radius_m,
            slip_exclusion_m: value.slip_exclusion_m,
            hill_radius_m: value.hill_radius_m,
        }
    }
}

impl TryFrom<&abi::LocalObstacle> for crate::LocalObstacle {
    type Error = ();

    fn try_from(value: &abi::LocalObstacle) -> Result<Self, ()> {
        Ok(Self {
            reference: (&value.reference).try_into()?,
            pose: (&value.pose).into(),
            radius_m: value.radius_m,
            slip_exclusion_m: value.slip_exclusion_m,
            hill_radius_m: value.hill_radius_m,
        })
    }
}

impl From<&travel::FuelRequirement> for abi::FuelRequirement {
    fn from(value: &travel::FuelRequirement) -> Self {
        Self {
            resource: Text::new(&value.resource),
            required_kg: value.required_kg,
            available_kg: value.available_kg,
        }
    }
}

impl TryFrom<&abi::FuelRequirement> for travel::FuelRequirement {
    type Error = ();

    fn try_from(value: &abi::FuelRequirement) -> Result<Self, ()> {
        Ok(Self {
            resource: text(&value.resource)?,
            required_kg: value.required_kg,
            available_kg: value.available_kg,
        })
    }
}

impl From<&travel::CelestialRef> for abi::CelestialRef {
    fn from(value: &travel::CelestialRef) -> Self {
        Self {
            system: value.system.0,
            body: value.body.0,
        }
    }
}

impl From<&abi::CelestialRef> for travel::CelestialRef {
    fn from(value: &abi::CelestialRef) -> Self {
        Self {
            system: Id(value.system),
            body: Id(value.body),
        }
    }
}

impl TryFrom<&travel::FirmwareStatus> for abi::FirmwareStatus {
    type Error = ();

    fn try_from(value: &travel::FirmwareStatus) -> Result<Self, ()> {
        if value.markers.len() > abi::MAX_STATUS_MARKERS
            || value.summary.len() > 256
            || value.markers.iter().any(|marker| marker.label.len() > 64)
        {
            return Err(());
        }
        let (phase, until, why) = match &value.phase {
            travel::FirmwarePhase::Idle => (0, None, ""),
            travel::FirmwarePhase::Planning => (1, None, ""),
            travel::FirmwarePhase::Waiting { until, why } => (2, *until, why.as_str()),
            travel::FirmwarePhase::Charging => (3, None, ""),
            travel::FirmwarePhase::Transit => (4, None, ""),
            travel::FirmwarePhase::Maneuvering => (5, None, ""),
            travel::FirmwarePhase::Docking => (6, None, ""),
            travel::FirmwarePhase::Completed => (7, None, ""),
        };
        if why.len() > 256 {
            return Err(());
        }
        let mut output = Self {
            phase,
            waiting_until_present: until.is_some() as u64,
            waiting_until: until.unwrap_or_default(),
            waiting_reason: Text::new(why),
            summary: Text::new(&value.summary),
            arrival_present: value.estimated_arrival_tick.is_some() as u64,
            arrival_tick: value.estimated_arrival_tick.unwrap_or_default(),
            capture_present: value.capture_body.is_some() as u64,
            capture_body: value
                .capture_body
                .as_ref()
                .map(Into::into)
                .unwrap_or_default(),
            aim_present: value.aim_offset_m.is_some() as u64,
            aim_offset_m: value.aim_offset_m.unwrap_or_default(),
            departure_present: value.departure_tick.is_some() as u64,
            departure_tick: value.departure_tick.unwrap_or_default(),
            planned_delta_v_m_s: value.planned_delta_v_m_s,
            planned_loss_ppm: value.planned_loss_ppm,
            spent_loss_ppm: value.spent_loss_ppm,
            planned_exotic_fuel_kg: value.planned_exotic_fuel_kg,
            spent_exotic_fuel_kg: value.spent_exotic_fuel_kg,
            marker_count: value.markers.len() as u64,
            ..Default::default()
        };
        for (out, marker) in output.markers.iter_mut().zip(&value.markers) {
            *out = abi::PlanMarker {
                position: position_record(&marker.position),
                label: Text::new(&marker.label),
            };
        }
        Ok(output)
    }
}

impl TryFrom<&abi::FirmwareStatus> for travel::FirmwareStatus {
    type Error = ();

    fn try_from(value: &abi::FirmwareStatus) -> Result<Self, ()> {
        if !value.planned_delta_v_m_s.is_finite()
            || !value.planned_exotic_fuel_kg.is_finite()
            || value.planned_exotic_fuel_kg < 0.0
            || !value.spent_exotic_fuel_kg.is_finite()
            || value.spent_exotic_fuel_kg < 0.0
            || value.planned_delta_v_m_s < 0.0
            || !value.planned_loss_ppm.is_finite()
            || !(0.0..=1_000_000.0).contains(&value.planned_loss_ppm)
            || !value.spent_loss_ppm.is_finite()
            || !(0.0..=1_000_000.0).contains(&value.spent_loss_ppm)
            || value.aim_offset_m.iter().any(|x| !x.is_finite())
        {
            return Err(());
        }
        Ok(Self {
            phase: match value.phase {
                0 => travel::FirmwarePhase::Idle,
                1 => travel::FirmwarePhase::Planning,
                2 => travel::FirmwarePhase::Waiting {
                    until: optional(value.waiting_until_present, value.waiting_until)?,
                    why: text(&value.waiting_reason)?,
                },
                3 => travel::FirmwarePhase::Charging,
                4 => travel::FirmwarePhase::Transit,
                5 => travel::FirmwarePhase::Maneuvering,
                6 => travel::FirmwarePhase::Docking,
                7 => travel::FirmwarePhase::Completed,
                _ => return Err(()),
            },
            summary: text(&value.summary)?,
            estimated_arrival_tick: optional(value.arrival_present, value.arrival_tick)?,
            capture_body: optional(value.capture_present, (&value.capture_body).into())?,
            aim_offset_m: optional(value.aim_present, value.aim_offset_m)?,
            departure_tick: optional(value.departure_present, value.departure_tick)?,
            planned_delta_v_m_s: value.planned_delta_v_m_s,
            planned_loss_ppm: value.planned_loss_ppm,
            spent_loss_ppm: value.spent_loss_ppm,
            planned_exotic_fuel_kg: value.planned_exotic_fuel_kg,
            spent_exotic_fuel_kg: value.spent_exotic_fuel_kg,
            markers: counted(&value.markers, value.marker_count)?
                .iter()
                .map(|marker| {
                    Ok(travel::PlanMarker {
                        position: position_value(&marker.position),
                        label: text(&marker.label)?,
                    })
                })
                .collect::<Result<_, ()>>()?,
        })
    }
}

pub fn travel_reply(
    value: &abi::TravelReply,
    itinerary: &[abi::ItineraryEntry],
    fuels: &[abi::FuelRequirement],
    hierarchy: &[abi::CelestialRef],
) -> Result<ProgramReply, ()> {
    use crate::location::{LocationContext, LocationRegion};
    let presence = match value.presence {
        0 => travel::Presence::Space,
        1 => travel::Presence::Docked {
            host: Id(value.host),
            bay: value.bay.try_into().map_err(|_| ())?,
        },
        2 => travel::Presence::SlipTransit(Id(value.host)),
        3 => travel::Presence::StoredInWreck(Id(value.host)),
        4 => travel::Presence::Destroyed,
        _ => return Err(()),
    };
    let fuel_budget = travel::FuelBudget {
        resources: counted(fuels, value.fuel_count)?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
        complete: flag(value.fuel_complete)?,
    };
    Ok(ProgramReply::Travel {
        state: travel::AutopilotState {
            enabled: flag(value.enabled)?,
            directive_revision: value.directive_revision,
            itinerary: counted(itinerary, value.itinerary_count)?
                .iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            preferences: (&value.preferences).try_into()?,
            risk_budget: travel::RiskBudget {
                max_log_loss: value.max_log_loss,
                spent_log_loss: value.spent_log_loss,
            },
            fuel_budget: optional(value.fuel_present, fuel_budget)?,
            status: (&value.status).try_into()?,
            failure: optional(value.failure_present, text(&value.failure)?)?,
        },
        pose: (&value.pose).into(),
        presence,
        location: LocationContext {
            region: match value.region {
                0 => LocationRegion::System,
                1 => LocationRegion::Interstellar,
                2 => LocationRegion::SlipTransit,
                _ => return Err(()),
            },
            system: optional(value.system_present, Id(value.system))?,
            primary: optional(value.primary_present, (&value.primary).into())?,
            hierarchy: counted(hierarchy, value.hierarchy_count)?
                .iter()
                .map(Into::into)
                .collect(),
            sample_tick: value.sample_tick,
        },
        tick: value.tick,
        exotic_fuel_kg: value.exotic_fuel_kg,
        slip_ready: flag(value.slip_ready)?,
        slip_axis: value.slip_axis,
    })
}

fn counted<T>(values: &[T], count: u64) -> Result<&[T], ()> {
    values
        .get(..usize::try_from(count).map_err(|_| ())?)
        .ok_or(())
}

impl From<&abi::OrreryQuery> for ProgramQuery {
    fn from(value: &abi::OrreryQuery) -> Self {
        Self::Orrery {
            reference: position_value(&value.reference),
        }
    }
}

impl From<&abi::OrrerySystemQuery> for ProgramQuery {
    fn from(value: &abi::OrrerySystemQuery) -> Self {
        Self::OrrerySystem {
            system: Id(value.system),
            after_seconds: value.after_seconds,
        }
    }
}

impl TryFrom<&abi::SlipEligibilityQuery> for crate::SlipProbe {
    type Error = ();

    fn try_from(value: &abi::SlipEligibilityQuery) -> Result<Self, ()> {
        Ok(Self {
            arrival_velocity: optional(value.arrival_velocity_present, value.arrival_velocity)?,
            origin: position_value(&value.origin),
            destination: position_value(&value.destination),
            departure_after_seconds: value.departure_after_seconds,
            arrival_after_seconds: value.arrival_after_seconds,
            navigation_beacon: optional(
                value.navigation_beacon_present,
                Id(value.navigation_beacon),
            )?,
        })
    }
}

impl From<&crate::SlipProbe> for abi::SlipEligibilityQuery {
    fn from(value: &crate::SlipProbe) -> Self {
        Self {
            arrival_velocity_present: value.arrival_velocity.is_some() as u64,
            arrival_velocity: value.arrival_velocity.unwrap_or_default(),
            origin: position_record(&value.origin),
            destination: position_record(&value.destination),
            departure_after_seconds: value.departure_after_seconds,
            arrival_after_seconds: value.arrival_after_seconds,
            navigation_beacon_present: value.navigation_beacon.is_some() as u64,
            navigation_beacon: value.navigation_beacon.unwrap_or_default().0,
        }
    }
}

impl From<&crate::SlipProbeResult> for abi::SlipProbeReply {
    fn from(value: &crate::SlipProbeResult) -> Self {
        Self {
            result: abi::SlipEligibilityReply {
                ready: value.ready as u64,
                preparation_s: value.preparation_s,
                duration_s: value.duration_s,
            },
            error: Text::new(value.error.as_deref().unwrap_or_default()),
        }
    }
}

impl TryFrom<&abi::SlipProbeReply> for crate::SlipProbeResult {
    type Error = ();

    fn try_from(value: &abi::SlipProbeReply) -> Result<Self, ()> {
        let error = text(&value.error)?;
        Ok(Self {
            ready: flag(value.result.ready)?,
            preparation_s: value.result.preparation_s,
            duration_s: value.result.duration_s,
            error: (!error.is_empty()).then_some(error),
        })
    }
}

impl TryFrom<&abi::SlipEligibilityQuery> for ProgramQuery {
    type Error = ();

    fn try_from(value: &abi::SlipEligibilityQuery) -> Result<Self, ()> {
        Ok(Self::SlipEligibility {
            origin: position_value(&value.origin),
            destination: position_value(&value.destination),
            departure_after_seconds: value.departure_after_seconds,
            arrival_after_seconds: value.arrival_after_seconds,
            navigation_beacon: optional(
                value.navigation_beacon_present,
                Id(value.navigation_beacon),
            )?,
        })
    }
}

impl TryFrom<&abi::ResolveQuery> for ProgramQuery {
    type Error = ();

    fn try_from(value: &abi::ResolveQuery) -> Result<Self, ()> {
        Ok(Self::Resolve {
            destination: (&value.destination).try_into()?,
            after_seconds: value.after_seconds,
        })
    }
}

macro_rules! action {
    ($ty:ident, $value:ident, $body:expr) => {
        impl TryFrom<&abi::$ty> for ProgramAction {
            type Error = ();

            fn try_from($value: &abi::$ty) -> Result<Self, ()> {
                Ok($body)
            }
        }
    };
}

action!(
    Fail,
    value,
    Self::Fail {
        directive_revision: value.directive_revision,
        reason: text(&value.reason)?
    }
);
action!(
    SetAutopilot,
    value,
    Self::SetAutopilot {
        directive_revision: value.directive_revision,
        enabled: flag(value.enabled)?
    }
);
action!(
    ClearItinerary,
    value,
    Self::ClearItinerary {
        directive_revision: value.directive_revision
    }
);
action!(
    PublishStatus,
    value,
    Self::PublishStatus {
        directive_revision: value.directive_revision,
        status: (&value.status).try_into()?
    }
);
action!(
    Complete,
    value,
    Self::Complete {
        directive_revision: value.directive_revision
    }
);
action!(
    Slip,
    value,
    Self::Slip {
        destination: position_value(&value.destination),
        navigation_beacon: optional(value.navigation_beacon_present, Id(value.navigation_beacon))?,
        arrival_velocity: optional(value.arrival_velocity_present, value.arrival_velocity)?,
        not_before_tick: optional(value.not_before_present, value.not_before_tick)?,
    }
);
action!(
    ReserveBay,
    value,
    Self::ReserveBay {
        station: Id(value.station),
        bay: value.bay.try_into().map_err(|_| ())?,
    }
);
action!(
    Dock,
    value,
    Self::Dock {
        station: Id(value.station),
        bay: value.bay.try_into().map_err(|_| ())?,
    }
);
action!(Undock, _value, Self::Undock);

impl From<&abi::ContactRef> for ProgramQuery {
    fn from(value: &abi::ContactRef) -> Self {
        Self::Contact(value.into())
    }
}

impl TryFrom<&ProgramReply> for abi::TravelReply {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        let ProgramReply::Travel {
            state,
            pose,
            presence,
            location,
            tick,
            exotic_fuel_kg,
            slip_ready,
            slip_axis,
        } = value
        else {
            return Err(());
        };
        let (presence, host, bay) = match presence {
            travel::Presence::Space => (0, Id::default(), 0),
            travel::Presence::Docked { host, bay } => (1, *host, *bay as u64),
            travel::Presence::SlipTransit(id) => (2, *id, 0),
            travel::Presence::StoredInWreck(id) => (3, *id, 0),
            travel::Presence::Destroyed => (4, Id::default(), 0),
        };
        Ok(Self {
            enabled: state.enabled as u64,
            preferences: (&state.preferences).into(),
            directive_revision: state.directive_revision,
            itinerary_count: state.itinerary.len() as u64,
            fuel_present: state.fuel_budget.is_some() as u64,
            fuel_count: state
                .fuel_budget
                .as_ref()
                .map_or(0, |budget| budget.resources.len()) as u64,
            fuel_complete: state
                .fuel_budget
                .as_ref()
                .is_some_and(|budget| budget.complete) as u64,
            max_log_loss: state.risk_budget.max_log_loss,
            spent_log_loss: state.risk_budget.spent_log_loss,
            status: (&state.status).try_into()?,
            failure_present: state.failure.is_some() as u64,
            failure: Text::new(state.failure.as_deref().unwrap_or_default()),
            pose: pose.into(),
            slip_ready: *slip_ready as u64,
            slip_axis: *slip_axis,
            presence,
            host: host.0,
            bay,
            region: match location.region {
                crate::location::LocationRegion::System => 0,
                crate::location::LocationRegion::Interstellar => 1,
                crate::location::LocationRegion::SlipTransit => 2,
            },
            system_present: location.system.is_some() as u64,
            system: location.system.unwrap_or_default().0,
            primary_present: location.primary.is_some() as u64,
            primary: location
                .primary
                .as_ref()
                .map(Into::into)
                .unwrap_or_default(),
            hierarchy_count: location.hierarchy.len() as u64,
            sample_tick: location.sample_tick,
            tick: *tick,
            exotic_fuel_kg: *exotic_fuel_kg,
        })
    }
}

impl TryFrom<&ProgramReply> for abi::ContactReply {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::Contact {
                pose,
                handle,
                radius_m,
            } => Ok(Self {
                pose: pose.into(),
                handle: *handle,
                radius_m: *radius_m,
            }),
            _ => Err(()),
        }
    }
}

impl TryFrom<&ProgramReply> for abi::SlipEligibilityReply {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::SlipEligibility {
                ready,
                preparation_s,
                duration_s,
            } => Ok(Self {
                ready: *ready as u64,
                preparation_s: *preparation_s,
                duration_s: *duration_s,
            }),
            _ => Err(()),
        }
    }
}

impl TryFrom<&ProgramReply> for abi::Pose {
    type Error = ();

    fn try_from(value: &ProgramReply) -> Result<Self, ()> {
        match value {
            ProgramReply::Pose(pose) => Ok(pose.into()),
            _ => Err(()),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn syscall(status: i32) -> Result<(), i32> {
    if status < 0 { Err(status) } else { Ok(()) }
}

#[cfg(target_arch = "wasm32")]
pub fn query(query: &ProgramQuery) -> Result<ProgramReply, i32> {
    use abi::raw;
    use osg_ship_api::abi::ERR_ARGUMENT;

    match query {
        ProgramQuery::Orrery { .. } | ProgramQuery::OrrerySystem { .. } => {
            let mut values =
                vec![abi::LocalObstacle::default(); crate::local_space::MAX_LOCAL_OBSTACLES];
            let mut header = abi::OrreryReply::default();
            syscall(unsafe {
                match query {
                    ProgramQuery::Orrery { reference } => raw::orrery_read(
                        &abi::OrreryQuery {
                            reference: position_record(reference),
                        },
                        values.as_mut_ptr(),
                        values.len() as u32,
                        &mut header,
                    ),
                    ProgramQuery::OrrerySystem {
                        system,
                        after_seconds,
                    } => raw::orrery_system_read(
                        &abi::OrrerySystemQuery {
                            system: system.0,
                            after_seconds: *after_seconds,
                        },
                        values.as_mut_ptr(),
                        values.len() as u32,
                        &mut header,
                    ),
                    _ => unreachable!(),
                }
            })?;
            let obstacles = counted(&values, header.count)
                .map_err(|_| ERR_ARGUMENT)?
                .iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()
                .map_err(|_| ERR_ARGUMENT)?;
            Ok(ProgramReply::Orrery(obstacles))
        }
        ProgramQuery::Contact(contact) => {
            let mut output = abi::ContactReply::default();
            syscall(unsafe { raw::contact_get(&contact.into(), &mut output) })?;
            Ok(ProgramReply::Contact {
                pose: (&output.pose).into(),
                handle: output.handle,
                radius_m: output.radius_m,
            })
        }
        ProgramQuery::SlipEligibility {
            origin,
            destination,
            departure_after_seconds,
            arrival_after_seconds,
            navigation_beacon,
        } => {
            let input = abi::SlipEligibilityQuery {
                arrival_velocity_present: 0,
                arrival_velocity: [0.0; 3],
                origin: position_record(origin),
                destination: position_record(destination),
                departure_after_seconds: *departure_after_seconds,
                arrival_after_seconds: *arrival_after_seconds,
                navigation_beacon_present: navigation_beacon.is_some() as u64,
                navigation_beacon: navigation_beacon.unwrap_or_default().0,
            };
            let mut output = abi::SlipEligibilityReply::default();
            syscall(unsafe { raw::slip_eligibility(&input, &mut output) })?;
            Ok(ProgramReply::SlipEligibility {
                ready: flag(output.ready).map_err(|_| ERR_ARGUMENT)?,
                preparation_s: output.preparation_s,
                duration_s: output.duration_s,
            })
        }
        ProgramQuery::Travel => {
            let mut output = abi::TravelReply::default();
            let mut itinerary = vec![abi::ItineraryEntry::default(); travel::MAX_DIRECTIVES];
            let mut fuels = vec![abi::FuelRequirement::default(); 256];
            let mut hierarchy =
                vec![abi::CelestialRef::default(); crate::local_space::MAX_LOCAL_OBSTACLES];
            syscall(unsafe {
                raw::travel_read(
                    &mut output,
                    itinerary.as_mut_ptr(),
                    itinerary.len() as u32,
                    fuels.as_mut_ptr(),
                    fuels.len() as u32,
                    hierarchy.as_mut_ptr(),
                    hierarchy.len() as u32,
                )
            })?;
            travel_reply(&output, &itinerary, &fuels, &hierarchy).map_err(|_| ERR_ARGUMENT)
        }
        ProgramQuery::SlipEligibilityBatch(probes) => {
            let inputs: Vec<abi::SlipEligibilityQuery> = probes.iter().map(Into::into).collect();
            let mut outputs = vec![abi::SlipProbeReply::default(); probes.len()];
            syscall(unsafe {
                raw::slip_eligibility_batch(
                    inputs.as_ptr(),
                    inputs.len() as u32,
                    outputs.as_mut_ptr(),
                )
            })?;
            Ok(ProgramReply::SlipEligibilityBatch(
                outputs
                    .iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()
                    .map_err(|_| ERR_ARGUMENT)?,
            ))
        }
        ProgramQuery::Resolve {
            destination,
            after_seconds,
        } => {
            let input = abi::ResolveQuery {
                destination: destination.into(),
                after_seconds: *after_seconds,
            };
            let mut output = abi::Pose::default();
            syscall(unsafe { raw::destination_resolve(&input, &mut output) })?;
            Ok(ProgramReply::Pose((&output).into()))
        }
        _ => crate::wasm_beacons::query(query),
    }
}

#[cfg(target_arch = "wasm32")]
pub fn command(action: ProgramAction) -> Result<(), i32> {
    use abi::raw;
    use osg_ship_api::abi::ERR_ARGUMENT;

    let result = match action {
        ProgramAction::SetAutopilot {
            directive_revision,
            enabled,
        } => unsafe {
            raw::travel_set_autopilot(&abi::SetAutopilot {
                directive_revision,
                enabled: enabled as u64,
            })
        },
        ProgramAction::ClearItinerary { directive_revision } => unsafe {
            raw::travel_clear_itinerary(&abi::ClearItinerary { directive_revision })
        },
        ProgramAction::Fail {
            directive_revision,
            reason,
        } => {
            if reason.len() > 256 {
                return Err(ERR_ARGUMENT);
            }
            unsafe {
                raw::travel_fail(&abi::Fail {
                    directive_revision,
                    reason: Text::new(&reason),
                })
            }
        }
        ProgramAction::PublishStatus {
            directive_revision,
            status,
        } => unsafe {
            raw::travel_publish_status(&abi::PublishStatus {
                directive_revision,
                status: (&status).try_into().map_err(|_| ERR_ARGUMENT)?,
            })
        },
        ProgramAction::Complete { directive_revision } => unsafe {
            raw::travel_complete(&abi::Complete { directive_revision })
        },
        ProgramAction::Slip {
            destination,
            navigation_beacon,
            arrival_velocity,
            not_before_tick,
        } => unsafe {
            raw::travel_slip(&abi::Slip {
                destination: position_record(&destination),
                navigation_beacon_present: navigation_beacon.is_some() as u64,
                navigation_beacon: navigation_beacon.unwrap_or_default().0,
                arrival_velocity_present: arrival_velocity.is_some() as u64,
                arrival_velocity: arrival_velocity.unwrap_or_default(),
                not_before_present: not_before_tick.is_some() as u64,
                not_before_tick: not_before_tick.unwrap_or_default(),
            })
        },
        ProgramAction::ReserveBay { station, bay } => unsafe {
            raw::travel_reserve_bay(&abi::ReserveBay {
                station: station.0,
                bay: bay as u64,
            })
        },
        ProgramAction::Dock { station, bay } => unsafe {
            raw::travel_dock(&abi::Dock {
                station: station.0,
                bay: bay as u64,
            })
        },
        ProgramAction::Undock => unsafe { raw::travel_undock(&abi::Undock::default()) },
        ProgramAction::CancelSlip => unsafe { raw::travel_cancel_slip() },
    };
    syscall(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_ship_api::abi::Record;

    #[test]
    fn wide_coordinates_survive_record_memory_roundtrip() {
        let position = GalacticPosition::new(i128::MIN, i128::MAX, -(1_i128 << 90) + 17);
        let record = position_record(&position);
        let restored = abi::Position::read(record.bytes()).unwrap();
        assert_eq!(position_value(&restored), position);
    }

    #[test]
    fn travel_publication_preserves_context_and_rejects_short_arrays() {
        let body = travel::CelestialRef {
            system: Id([1; 16]),
            body: Id([2; 16]),
        };
        let marker = travel::PlanMarker {
            position: GalacticPosition::new(1_i128 << 100, -17, 31),
            label: "Capture".into(),
        };
        let state = travel::AutopilotState {
            enabled: true,
            directive_revision: 77,
            itinerary: vec![travel::ItineraryEntry {
                directive: travel::Directive::DockAt(Id([3; 16])),
                label: "Station".into(),
            }],
            status: travel::FirmwareStatus {
                phase: travel::FirmwarePhase::Waiting {
                    until: Some(500),
                    why: "Planet occlusion".into(),
                },
                summary: "Awaiting departure".into(),
                capture_body: Some(body),
                aim_offset_m: Some([100.0, 200.0, -50.0]),
                departure_tick: Some(500),
                markers: vec![marker],
                ..Default::default()
            },
            ..Default::default()
        };
        let location = crate::location::LocationContext {
            region: crate::location::LocationRegion::System,
            system: Some(body.system),
            primary: Some(body),
            hierarchy: vec![body],
            sample_tick: 123,
        };
        let reply = ProgramReply::Travel {
            state: state.clone(),
            pose: crate::Pose::default(),
            presence: travel::Presence::Docked {
                host: Id([3; 16]),
                bay: 2,
            },
            location: location.clone(),
            tick: 124,
            exotic_fuel_kg: 200.0,
            slip_ready: false,
            slip_axis: [0.0, 0.0, 1.0],
        };
        let header = abi::TravelReply::try_from(&reply).unwrap();
        let itinerary: Vec<_> = state
            .itinerary
            .iter()
            .map(abi::ItineraryEntry::from)
            .collect();
        let hierarchy: Vec<_> = location
            .hierarchy
            .iter()
            .map(abi::CelestialRef::from)
            .collect();
        let ProgramReply::Travel {
            state: decoded,
            location: decoded_location,
            presence,
            tick,
            ..
        } = travel_reply(&header, &itinerary, &[], &hierarchy).unwrap()
        else {
            panic!("travel reply");
        };
        assert_eq!(decoded, state);
        assert_eq!(decoded_location, location);
        assert_eq!(
            presence,
            travel::Presence::Docked {
                host: Id([3; 16]),
                bay: 2
            }
        );
        assert_eq!(tick, 124);
        assert!(travel_reply(&header, &[], &[], &hierarchy).is_err());
        assert!(travel_reply(&header, &itinerary, &[], &[]).is_err());
        let mut malformed = header;
        malformed.status.marker_count = abi::MAX_STATUS_MARKERS as u64 + 1;
        assert!(travel_reply(&malformed, &itinerary, &[], &hierarchy).is_err());
    }

    #[test]
    fn malformed_discriminants_and_text_are_rejected() {
        let destination = abi::Destination {
            kind: 999,
            ..Default::default()
        };
        assert!(travel::Destination::try_from(&destination).is_err());
        let preferences = abi::Preferences {
            fuel_fraction: 0.5,
            max_loss_ppm: f64::NAN,
            allow_slipdrive: 1,
        };
        assert!(travel::PlanningPreferences::try_from(&preferences).is_err());
        let mut reason = Text::<256>::new("valid");
        reason.len = 257;
        assert!(text(&reason).is_err());
        reason.len = 1;
        reason.bytes[0] = 255;
        assert!(text(&reason).is_err());
    }

    #[test]
    fn slip_probe_and_launch_preserve_arrival_velocity_and_timing() {
        let probe = crate::SlipProbe {
            origin: GalacticPosition::new(1_i128 << 100, 0, 0),
            destination: GalacticPosition::new(-(1_i128 << 95), 42, 7),
            departure_after_seconds: 120.0,
            arrival_after_seconds: 900.0,
            navigation_beacon: Some(Id([9; 16])),
            arrival_velocity: Some([1000.0, -2000.0, 3000.0]),
        };
        let restored =
            crate::SlipProbe::try_from(&abi::SlipEligibilityQuery::from(&probe)).unwrap();
        assert_eq!(restored.origin, probe.origin);
        assert_eq!(restored.destination, probe.destination);
        assert_eq!(restored.arrival_velocity, probe.arrival_velocity);
        assert_eq!(restored.departure_after_seconds, 120.0);
        let launch = abi::Slip {
            destination: position_record(&probe.destination),
            arrival_velocity_present: 1,
            arrival_velocity: probe.arrival_velocity.unwrap(),
            not_before_present: 1,
            not_before_tick: 1200,
            ..Default::default()
        };
        let ProgramAction::Slip {
            arrival_velocity,
            not_before_tick,
            ..
        } = ProgramAction::try_from(&launch).unwrap()
        else {
            panic!("slip action");
        };
        assert_eq!(arrival_velocity, probe.arrival_velocity);
        assert_eq!(not_before_tick, Some(1200));
    }
}
