use super::*;
use std::collections::BTreeSet;

fn indexed<K: Ord>(commands: &mut Commands, index: &mut BTreeMap<K, Entity>, key: K) -> Entity {
    *index
        .entry(key)
        .or_insert_with(|| commands.spawn(WorldMember).id())
}

fn retain<K: Ord>(commands: &mut Commands, index: &mut BTreeMap<K, Entity>, seen: &BTreeSet<K>) {
    index.retain(|key, entity| {
        if seen.contains(key) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
}

fn samples(previous: Option<&Pose>, current: &Pose) -> (PoseSamples, DisplayPose) {
    let previous = previous.unwrap_or(current).clone();
    (
        PoseSamples {
            previous,
            current: current.clone(),
        },
        DisplayPose(current.clone()),
    )
}

pub(super) fn reset(
    mut commands: Commands,
    playback: Res<BufferedPlayback>,
    mut replication: ResMut<Replication>,
    mut clock: ResMut<RenderTime>,
    mut session: ResMut<SessionInfo>,
    members: Query<Entity, With<WorldMember>>,
) {
    if session.world == playback.0.world {
        return;
    }
    for entity in &members {
        commands.entity(entity).despawn();
    }
    *replication = Replication::default();
    *clock = RenderTime::default();
    *session = SessionInfo {
        world: playback.0.world,
        generation: session.generation + 1,
        status: std::mem::take(&mut session.status),
        ..default()
    };
    commands.trigger(SessionReset);
}

pub(super) fn apply(
    mut commands: Commands,
    mut playback: ResMut<BufferedPlayback>,
    mut replication: ResMut<Replication>,
    mut clock: ResMut<RenderTime>,
    mut info: ResMut<SessionInfo>,
    old_samples: Query<(&SpatialInstance, &PoseSamples, Option<&VisualSamples>)>,
    old_optical: Query<&Optical>,
) {
    let playback = &mut playback.0;
    let advanced = playback.tick().is_some();
    info.target_frames = playback.target_frames;
    info.underruns = playback.underruns;
    if !advanced {
        return;
    }
    let sequence = playback.frame().unwrap().sequence;
    let publications = playback.take_publications(sequence);
    let frame = playback.frame().unwrap();
    clock.previous_ns = if info.sequence == 0 {
        frame.sim_time_ns
    } else {
        clock.current_ns
    };
    clock.current_ns = frame.sim_time_ns;
    info.tick = frame.tick;
    info.sequence = frame.sequence;
    replication.events.retain(|_, (entity, timestamp)| {
        if clock.display_ns.saturating_sub(*timestamp) > 10_000_000_000 {
            commands.entity(*entity).despawn();
            false
        } else {
            true
        }
    });
    info.capabilities = frame.presentation.capabilities.clone();
    info.groups = frame.tracks.keys().copied().collect();
    info.diagnostics = frame.presentation.diagnostics.clone();
    info.universe = frame.presentation.universe.clone();
    info.navigation_ephemerides = frame.presentation.navigation.ephemerides.clone();
    let catalogue = frame.presentation.navigation.catalogue;
    if info.navigation_hash != catalogue {
        info.navigation_hash = catalogue;
        info.navigation = Default::default();
        info.navigation_status = if catalogue.is_some() {
            NavigationStatus::Loading
        } else {
            NavigationStatus::Unavailable
        };
    }
    info.society = frame.society.clone();
    for industry in publications.industry {
        info.industry.apply(industry);
    }
    for chat in publications.chat {
        info.chat.apply(chat);
    }
    info.events.extend(publications.events);
    let excess = info.events.len().saturating_sub(128);
    info.events.drain(..excess);
    info.results.extend(publications.results);
    let excess = info.results.len().saturating_sub(128);
    info.results.drain(..excess);

    let mut seen_beacons = BTreeSet::new();
    for beacon in &frame.presentation.navigation.beacons {
        seen_beacons.insert(beacon.id);
        let entity = indexed(&mut commands, &mut replication.beacons, beacon.id);
        let old = old_samples
            .get(entity)
            .ok()
            .map(|(_, poses, _)| &poses.current);
        commands.entity(entity).insert((
            NavigationObject(beacon.clone()),
            SpatialInstance(beacon.id),
            samples(old, &beacon.pose),
        ));
    }
    retain(&mut commands, &mut replication.beacons, &seen_beacons);

    let mut seen = BTreeSet::new();
    for (group, tracks) in &frame.tracks {
        for track in tracks {
            let key = (*group, track.id);
            seen.insert(key);
            if let Some(entity) = replication.contacts.get(&key).copied() {
                if old_samples
                    .get(entity)
                    .is_ok_and(|(instance, _, _)| instance.0 != track.spatial_instance)
                {
                    commands.entity(entity).despawn();
                    replication.contacts.remove(&key);
                }
            }
            let entity = indexed(&mut commands, &mut replication.contacts, key);
            let old = old_samples.get(entity).ok();
            let reference = ContactRef {
                group: *group,
                track: track.id,
            };
            commands.entity(entity).insert((
                Contact(track.clone(), reference),
                SpatialInstance(track.spatial_instance),
                samples(old.map(|(_, poses, _)| &poses.current), &track.pose),
            ));
        }
    }
    retain(&mut commands, &mut replication.contacts, &seen);

    let mut seen = BTreeSet::new();
    for observation in &frame.optical {
        let key = (observation.view, observation.id);
        seen.insert(key);
        if let Some(entity) = replication.optical.get(&key).copied() {
            if old_samples
                .get(entity)
                .is_ok_and(|(instance, _, _)| instance.0 != observation.spatial_instance)
            {
                commands.entity(entity).despawn();
                replication.optical.remove(&key);
            }
        }
        let entity = indexed(&mut commands, &mut replication.optical, key);
        let old = old_samples
            .get(entity)
            .ok()
            .filter(|(instance, _, _)| instance.0 == observation.spatial_instance);
        let previous_visual = old
            .and_then(|(_, _, visual)| visual)
            .map(|visual| &visual.current)
            .unwrap_or(&observation.visual);
        commands.entity(entity).insert((
            Optical(observation.clone()),
            OpticalLight {
                previous: old_optical
                    .get(entity)
                    .map_or(observation.luminosity_w, |old| old.0.luminosity_w),
                current: observation.luminosity_w,
                display_w: observation.luminosity_w,
            },
            SpatialInstance(observation.spatial_instance),
            samples(old.map(|(_, poses, _)| &poses.current), &observation.pose),
            VisualSamples {
                previous: previous_visual.clone(),
                current: observation.visual.clone(),
            },
            DisplayVisual(observation.visual.clone()),
        ));
    }
    retain(&mut commands, &mut replication.optical, &seen);

    let details: BTreeMap<_, _> = frame
        .presentation
        .ships
        .iter()
        .map(|ship| (ship.ship, ship))
        .collect();
    let mut seen = BTreeSet::new();
    for ship in &frame.ships {
        seen.insert(ship.ship);
        let entity = indexed(&mut commands, &mut replication.ships, ship.ship);
        commands.entity(entity).insert(OwnedShip(ship.clone()));
        if let Some(pose) = ship.pose.as_ref() {
            let old = old_samples
                .get(entity)
                .ok()
                .filter(|(instance, _, _)| instance.0 == ship.spatial_instance)
                .map(|(_, poses, _)| &poses.current);
            commands
                .entity(entity)
                .insert((SpatialInstance(ship.spatial_instance), samples(old, pose)));
        } else {
            commands
                .entity(entity)
                .remove::<(SpatialInstance, PoseSamples, DisplayPose)>();
        }
        if let Some(details) = details.get(&ship.ship) {
            commands
                .entity(entity)
                .insert(ShipDetails((*details).clone()));
        } else {
            commands.entity(entity).remove::<ShipDetails>();
        }
    }
    retain(&mut commands, &mut replication.ships, &seen);

    let mut seen = BTreeSet::new();
    for view in &frame.views {
        seen.insert(view.id);
        let entity = indexed(&mut commands, &mut replication.views, view.id);
        let systems = frame
            .presentation
            .celestial_systems
            .iter()
            .filter(|system| system.view == view.id)
            .cloned()
            .collect();
        commands
            .entity(entity)
            .insert((ViewObservation(view.clone()), SystemSubscription(systems)));
    }
    retain(&mut commands, &mut replication.views, &seen);

    for event in publications.combat {
        let entity = commands
            .spawn((WorldMember, CombatPublication(event.clone())))
            .id();
        replication
            .events
            .insert(event.sequence, (entity, event.sim_time_ns));
    }
    while replication.events.len() > 4096 {
        if let Some((_, (entity, _))) = replication.events.pop_first() {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    #[test]
    fn navigation_ephemerides_do_not_expand_camera_subscriptions() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        let mut first = snapshot(1, group, track, 0.);
        first.views.push(ViewState {
            id: 41,
            revision: 1,
            group,
            focused_ship: None,
            origin: GalacticPosition::ZERO,
            tracks: vec![track],
            completion: Completion::Complete,
        });
        let reference = CelestialSystemRef {
            view: 41,
            system: Id([8; 16]),
            definition: [9; 32],
            epoch_mjd_utc: 0.,
            sim_time_origin_ns: 0,
        };
        std::sync::Arc::make_mut(&mut first.presentation.navigation).ephemerides =
            vec![reference.clone()];
        step(&mut app, 0.1, Some(first.clone()));
        assert_eq!(
            app.world().resource::<SessionInfo>().navigation_ephemerides,
            [reference]
        );
        let mut subscriptions = app.world_mut().query::<&SystemSubscription>();
        assert!(subscriptions.single(app.world()).unwrap().0.is_empty());

        first.tick = 2;
        first.sequence = 2;
        first.sim_time_ns = 200_000_000;
        std::sync::Arc::make_mut(&mut first.presentation.navigation)
            .ephemerides
            .clear();
        step(&mut app, 0.2, Some(first));
        assert!(
            app.world()
                .resource::<SessionInfo>()
                .navigation_ephemerides
                .is_empty()
        );
        assert!(subscriptions.single(app.world()).unwrap().0.is_empty());
    }

    #[test]
    fn live_beacons_replicate_independently_and_catalogue_change_clears_old_asset() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        let beacon = NavigationBeacon {
            id: Id([4; 16]),
            system: Id([5; 16]),
            name: "Live station".into(),
            pose: Pose::default(),
            radius_m: 100.,
            gate_exit: None,
            docking: true,
        };
        let mut first = snapshot(1, group, track, 0.);
        first.presentation.navigation = std::sync::Arc::new(NavigationSnapshot {
            catalogue: Some([6; 32]),
            beacons: vec![beacon.clone()],
            ephemerides: Vec::new(),
        });
        step(&mut app, 0.1, Some(first.clone()));
        assert_eq!(
            app.world().resource::<SessionInfo>().navigation_status,
            NavigationStatus::Loading
        );
        assert_eq!(
            app.world_mut()
                .query::<&NavigationObject>()
                .iter(app.world())
                .count(),
            1
        );
        let installed = std::sync::Arc::new(NavigationCatalogue {
            beacons: vec![beacon],
            ..Default::default()
        });
        app.world_mut().resource_mut::<SessionInfo>().navigation = installed.clone();
        let mut next = snapshot(2, group, track, 0.);
        next.presentation.navigation = first.presentation.navigation;
        step(&mut app, 0.2, Some(next));
        assert!(std::sync::Arc::ptr_eq(
            &installed,
            &app.world().resource::<SessionInfo>().navigation
        ));

        let mut replacement = snapshot(3, group, track, 0.);
        replacement.presentation.navigation = std::sync::Arc::new(NavigationSnapshot {
            catalogue: Some([7; 32]),
            beacons: Vec::new(),
            ephemerides: Vec::new(),
        });
        step(&mut app, 0.3, Some(replacement));
        assert!(
            app.world()
                .resource::<SessionInfo>()
                .navigation
                .beacons
                .is_empty()
        );
        assert_eq!(
            app.world().resource::<SessionInfo>().navigation_status,
            NavigationStatus::Loading
        );
        assert_eq!(
            app.world_mut()
                .query::<&NavigationObject>()
                .iter(app.world())
                .count(),
            0
        );
    }

    fn snapshot(sequence: u64, group: Id, track: Id, position: f64) -> Frame {
        let mut frame = Frame {
            industry: None,
            chat: None,
            optical: Vec::new(),
            calendar_unix_ms: 0,
            society: Default::default(),
            presentation: PresentationFrame::default(),
            world: Id([1; 16]),
            sequence,
            tick: sequence,
            sim_time_ns: sequence * 100_000_000,
            rate: 1.,
            views: Vec::new(),
            tracks: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
        };
        frame.tracks.insert(
            group,
            vec![Track {
                id: track,
                entity: None,
                spatial_instance: Id([5; 16]),
                pose: Pose {
                    position: GalacticPosition::ZERO.offset_by(glam::DVec3::X * position),
                    ..default()
                },
                position_sigma_m: 1.,
                velocity_sigma_m_s: 1.,
                observed_tick: sequence,
                estimate_tick: sequence,
                tags: default(),
                provenance: Provenance::Sensor,
                radius_m: Some(1.),
                appearance: None,
            }],
        );
        frame
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Time::<Fixed>::from_hz(10.))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
            .init_resource::<Replication>()
            .init_resource::<RenderTime>()
            .init_resource::<SessionInfo>()
            .insert_resource(BufferedPlayback(Playback::new(true)))
            .add_systems(FixedUpdate, (reset, apply).chain())
            .add_systems(Update, interpolate);
        app.update();
        app
    }

    fn step(app: &mut App, seconds: f64, frame: Option<Frame>) {
        let elapsed = app.world().resource::<Time<Real>>().elapsed();
        let delta = Duration::from_secs_f64(seconds) - elapsed;
        app.insert_resource(TimeUpdateStrategy::ManualDuration(delta));
        if let Some(frame) = frame {
            app.world_mut()
                .resource_mut::<BufferedPlayback>()
                .0
                .receive(frame)
                .unwrap();
        }
        app.update();
    }

    #[test]
    fn catchup_delivers_every_chat_page_in_sequence() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let mut outgoing = Outgoing::default();
        app.world_mut()
            .resource_mut::<SessionInfo>()
            .chat
            .subscribe(
                Some(ChatFocus {
                    view: 1,
                    view_revision: 7,
                    ship: Id([4; 16]),
                }),
                &mut outgoing,
            );

        for sequence in 2..=10 {
            let mut frame = snapshot(sequence, group, track, 0.);
            if sequence <= 3 {
                frame.chat = Some(toy_sim_model::chat::ChatUpdate {
                    subscription_revision: 1,
                    view: 1,
                    view_revision: 7,
                    unavailable: false,
                    page: toy_sim_model::chat::ChatPage {
                        messages: vec![toy_sim_model::chat::ChatMessage {
                            id: Id([sequence as u8; 16]),
                            sequence,
                            tick: sequence,
                            calendar_unix_ms: toy_sim_model::calendar::FOUR_HUNDRED_YEARS_MS,
                            sender_name: "Courier".into(),
                            advertised_owner: None,
                            advertised_organization: None,
                            text: format!("Message {sequence}"),
                        }],
                        next_sequence: sequence,
                        missed: 0,
                    },
                });
            }
            app.world_mut()
                .resource_mut::<BufferedPlayback>()
                .0
                .receive(frame)
                .unwrap();
        }
        step(&mut app, 0.2, None);
        let session = app.world().resource::<SessionInfo>();
        assert_eq!(session.sequence, 3);
        assert_eq!(
            session
                .chat
                .messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["Message 2", "Message 3"]
        );
    }

    #[test]
    fn catchup_preserves_industry_catalogue_and_following_permission_updates() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let mut outgoing = Outgoing::default();
        app.world_mut()
            .resource_mut::<SessionInfo>()
            .industry
            .subscribe(
                Some(industry::IndustrySubscription {
                    directory: true,
                    catalogue: true,
                    ..Default::default()
                }),
                &mut outgoing,
            );
        for sequence in 2..=10 {
            let mut frame = snapshot(sequence, group, track, 0.);
            if sequence <= 3 {
                frame.industry = Some(industry::IndustrySnapshot {
                    subscription_revision: 1,
                    catalogue: (sequence == 2).then_some(industry::IndustryCatalogue {
                        revision: [7; 32],
                        recipes: Vec::new(),
                        blueprints: Vec::new(),
                    }),
                    error: (sequence == 3).then(|| "Permission changed".into()),
                    ..Default::default()
                });
            }
            app.world_mut()
                .resource_mut::<BufferedPlayback>()
                .0
                .receive(frame)
                .unwrap();
        }
        step(&mut app, 0.2, None);
        let session = app.world().resource::<SessionInfo>();
        assert_eq!(session.sequence, 3);
        assert_eq!(
            session
                .industry
                .snapshot
                .catalogue
                .as_ref()
                .unwrap()
                .revision,
            [7; 32]
        );
        assert_eq!(
            session.industry.snapshot.error.as_deref(),
            Some("Permission changed")
        );
        app.world_mut()
            .resource_mut::<SessionInfo>()
            .industry
            .subscribe(None, &mut outgoing);
        assert_eq!(
            app.world().resource::<SessionInfo>().industry.snapshot,
            Default::default()
        );
    }

    #[test]
    fn contacts_are_stable_group_scoped_entities_with_interpolated_components() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        let mut first = snapshot(1, group, track, 0.);
        first.tracks.get_mut(&group).unwrap()[0].pose.velocity[0] = 10.;
        first.tracks.get_mut(&group).unwrap()[0]
            .pose
            .angular_velocity[0] = 0.2;
        step(&mut app, 0.1, Some(first));
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<Contact>>()
            .single(app.world())
            .unwrap();
        let mut second = snapshot(2, group, track, 100.);
        second.tracks.get_mut(&group).unwrap()[0].pose.velocity[0] = 30.;
        second.tracks.get_mut(&group).unwrap()[0]
            .pose
            .angular_velocity[0] = 0.6;
        let other_group = Id([4; 16]);
        second
            .tracks
            .insert(other_group, second.tracks[&group].clone());
        step(&mut app, 0.2, Some(second));
        step(&mut app, 0.25, None);
        assert_eq!(
            app.world_mut()
                .query::<&Contact>()
                .iter(app.world())
                .count(),
            2
        );
        assert_eq!(app.world().get::<Contact>(entity).unwrap().1.group, group);
        let position = app.world().get::<DisplayPose>(entity).unwrap().0.position;
        assert!((position.relative_to(GalacticPosition::ZERO).x - 50.).abs() < 1e-5);
        let pose = &app.world().get::<DisplayPose>(entity).unwrap().0;
        assert!((pose.velocity[0] - 20.).abs() < 1e-5);
        assert!((pose.angular_velocity[0] - 0.4).abs() < 1e-5);
    }

    #[test]
    fn new_spatial_instances_replace_observations_and_world_reset_removes_old_entities() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<Contact>>()
            .single(app.world())
            .unwrap();
        let mut next = snapshot(2, group, track, 1000.);
        next.tracks.get_mut(&group).unwrap()[0].spatial_instance = Id([6; 16]);
        step(&mut app, 0.2, Some(next));
        step(&mut app, 0.25, None);
        assert!(app.world().get_entity(entity).is_err());
        let arrived = app
            .world_mut()
            .query_filtered::<Entity, With<Contact>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            app.world()
                .get::<DisplayPose>(arrived)
                .unwrap()
                .0
                .position
                .relative_to(GalacticPosition::ZERO)
                .x,
            1000.
        );
        let mut reset = snapshot(1, group, track, 10.);
        reset.world = Id([9; 16]);
        step(&mut app, 0.3, Some(reset));
        assert!(app.world().get_entity(arrived).is_err());
        assert_eq!(
            app.world_mut()
                .query::<&CombatPublication>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query::<&Contact>()
                .iter(app.world())
                .count(),
            1
        );
    }

    #[test]
    fn empty_snapshot_removes_observations_without_recreating_them_on_display_frames() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let mut next = snapshot(2, group, track, 100.);
        next.tracks.clear();
        step(&mut app, 0.2, Some(next));
        step(&mut app, 0.25, None);
        assert_eq!(
            app.world_mut()
                .query::<&Contact>()
                .iter(app.world())
                .count(),
            0
        );
    }
    #[test]
    fn docking_removes_space_pose_and_capture_removes_private_telemetry() {
        let mut app = app();
        let mut first = snapshot(1, Id([2; 16]), Id([3; 16]), 0.);
        let ship = Id([4; 16]);
        first.ships.push(ShipTelemetry {
            appearance: None,
            radius_m: 10.,
            dock_services: Default::default(),
            info_group: InfoGroupKey([1; 32]),
            iff: IffIdentity {
                owner: Id([1; 16]),
                faction: None,
                labels: default(),
                enabled: true,
                range_m: 1e8,
            },
            ship,
            authority_revision: 1,
            spatial_instance: Id([5; 16]),
            presence: travel::Presence::Space,
            pose: Some(Pose::default()),
            battery_j: 0,
            hull_heat_j: 0.,
            shield_temperature_k: 0.,
            coolant_reserve_kg: 0.,
            travel: default(),
        });
        step(&mut app, 0.1, Some(first.clone()));
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<OwnedShip>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<DisplayPose>(entity).is_some());
        first.sequence = 2;
        first.tick = 2;
        first.sim_time_ns = 200_000_000;
        first.ships[0].pose = None;
        first.ships[0].presence = travel::Presence::Docked { host: ship, bay: 0 };
        step(&mut app, 0.2, Some(first.clone()));
        assert!(app.world().get::<OwnedShip>(entity).is_some());
        assert!(app.world().get::<DisplayPose>(entity).is_none());
        first.sequence = 3;
        first.tick = 3;
        first.sim_time_ns = 300_000_000;
        first.ships[0].presence = travel::Presence::Space;
        first.ships[0].spatial_instance = Id([6; 16]);
        first.ships[0].pose = Some(Pose {
            position: GalacticPosition::ZERO.offset_by(glam::DVec3::X * 1000.),
            ..default()
        });
        step(&mut app, 0.3, Some(first.clone()));
        step(&mut app, 0.35, None);
        assert_eq!(
            app.world().get::<SpatialInstance>(entity).unwrap().0,
            Id([6; 16])
        );
        assert_eq!(
            app.world().get::<DisplayPose>(entity).unwrap().0.position,
            first.ships[0].pose.as_ref().unwrap().position
        );

        first.sequence = 4;
        first.tick = 4;
        first.sim_time_ns = 400_000_000;
        first.ships.clear();
        step(&mut app, 0.4, Some(first));
        assert!(app.world().get_entity(entity).is_err());
    }

    #[test]
    fn underruns_replay_the_previous_interval_until_the_reserve_is_rebuilt() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        step(&mut app, 0.2, Some(snapshot(2, group, track, 100.)));
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<Contact>>()
            .single(app.world())
            .unwrap();

        for seconds in [0.25, 0.35, 0.45] {
            step(&mut app, seconds, None);
            let pose = &app.world().get::<DisplayPose>(entity).unwrap().0;
            assert!((pose.position.relative_to(GalacticPosition::ZERO).x - 50.).abs() < 1e-6);
            let clock = app.world().resource::<RenderTime>();
            assert_eq!(app.world().resource::<SessionInfo>().sequence, 2);
            assert_eq!(clock.display_ns, 150_000_000);
        }
        assert_eq!(app.world().resource::<SessionInfo>().underruns, 1);
        step(&mut app, 0.5, Some(snapshot(3, group, track, 200.)));
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 2);
        step(&mut app, 0.6, Some(snapshot(4, group, track, 300.)));
        step(&mut app, 0.65, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 3);
        let pose = &app.world().get::<DisplayPose>(entity).unwrap().0;
        assert!((pose.position.relative_to(GalacticPosition::ZERO).x - 150.).abs() < 1e-6);
    }

    #[test]
    fn fixed_schedule_consumes_once_per_tick_and_uses_authoritative_timestamps() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.05, Some(snapshot(1, group, track, 0.)));
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 0);
        step(&mut app, 0.1, None);
        assert_eq!(app.world().resource::<RenderTime>().display_ns, 100_000_000);

        let mut accelerated = snapshot(2, group, track, 100.);
        accelerated.sim_time_ns = 1_100_000_000;
        accelerated.tick = 11;
        accelerated.rate = 10.;
        step(&mut app, 0.2, Some(accelerated.clone()));
        step(&mut app, 0.25, None);
        assert_eq!(app.world().resource::<RenderTime>().display_ns, 600_000_000);

        let mut paused = accelerated.clone();
        paused.sequence = 3;
        paused.rate = 0.;
        step(&mut app, 0.3, Some(paused.clone()));
        step(&mut app, 0.35, None);
        assert_eq!(
            app.world().resource::<RenderTime>().display_ns,
            1_100_000_000
        );
        paused.sequence = 4;
        app.world_mut()
            .resource_mut::<BufferedPlayback>()
            .0
            .receive(paused.clone())
            .unwrap();
        paused.sequence = 5;
        app.world_mut()
            .resource_mut::<BufferedPlayback>()
            .0
            .receive(paused)
            .unwrap();
        step(&mut app, 0.55, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 5);
    }

    #[test]
    fn catchup_preserves_both_publications_and_interpolates_to_the_final_sample() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = Id([3; 16]);
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        for sequence in 2..=14 {
            let mut frame = snapshot(sequence, group, track, (sequence - 1) as f64 * 100.);
            if sequence <= 3 {
                frame.results.push(CommandResult {
                    id: Id((sequence as u128).to_le_bytes()),
                    effective_tick: sequence,
                    reply: None,
                    error: Some("test result".into()),
                });
                frame.events.push(toy_sim_model::Event {
                    sequence,
                    tick: sequence,
                    subject: None,
                    kind: format!("event {sequence}"),
                    position: None,
                });
                frame.presentation.combat.push(CombatEvent {
                    sequence,
                    sim_time_ns: frame.sim_time_ns,
                    kind: CombatEventKind::Destroyed {
                        target: ContactRef { group, track },
                        pose: Pose::default(),
                        appearance: None,
                        energy_j: 1.,
                        mass_kg: 1.,
                        radius_m: 1.,
                    },
                });
                frame.tracks.get_mut(&group).unwrap()[0].spatial_instance = Id([6; 16]);
            }
            app.world_mut()
                .resource_mut::<BufferedPlayback>()
                .0
                .receive(frame)
                .unwrap();
        }
        step(&mut app, 0.2, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 3);
        let session = app.world().resource::<SessionInfo>();
        assert_eq!(
            session
                .results
                .iter()
                .map(|r| r.effective_tick)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(
            session
                .events
                .iter()
                .map(|e| e.sequence)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(
            app.world_mut()
                .query::<&CombatPublication>()
                .iter(app.world())
                .count(),
            2
        );

        step(&mut app, 0.3, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 5);
        step(&mut app, 0.4, None);
        step(&mut app, 0.45, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 7);
        let pose = app
            .world_mut()
            .query_filtered::<&DisplayPose, With<Contact>>()
            .single(app.world())
            .unwrap();
        assert!((pose.0.position.relative_to(GalacticPosition::ZERO).x - 500.).abs() < 1e-6);
        assert_eq!(app.world().resource::<RenderTime>().display_ns, 600_000_000);
        assert_eq!(app.world().resource::<SessionInfo>().results.len(), 2);
    }
    fn optical(view: u64, position: f64, luminosity_w: f64) -> optical::OpticalObservation {
        optical::OpticalObservation {
            view,
            id: Id([9; 16]),
            spatial_instance: Id([10; 16]),
            known_entity: None,
            contact: None,
            pose: Pose {
                position: GalacticPosition::ZERO.offset_by(glam::DVec3::X * position),
                ..Default::default()
            },
            radius_m: 10.,
            luminosity_w,
            appearance: Some([11; 32]),
            visual: ShipVisual {
                engines: Vec::new(),
                turrets: Vec::new(),
                shield: None,
            },
        }
    }

    #[test]
    fn optical_entities_are_view_scoped_independent_of_radio_tracks_and_interpolate_light() {
        let mut app = app();
        let mut first = snapshot(1, Id([2; 16]), Id([3; 16]), 0.);
        first.optical = vec![optical(1, 0., 10.), optical(2, 1000., 20.)];
        step(&mut app, 0.1, Some(first));
        assert_eq!(
            app.world_mut()
                .query::<&Optical>()
                .iter(app.world())
                .count(),
            2
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<&DisplayVisual, With<Contact>>()
                .iter(app.world())
                .count(),
            0
        );
        let source = app
            .world_mut()
            .query::<(Entity, &Optical)>()
            .iter(app.world())
            .find(|(_, object)| object.0.view == 1)
            .unwrap()
            .0;

        let mut second = snapshot(2, Id([2; 16]), Id([3; 16]), 0.);
        second.tracks.clear();
        second.optical = vec![optical(1, 100., 30.), optical(2, 2000., 40.)];
        step(&mut app, 0.2, Some(second));
        step(&mut app, 0.25, None);
        let position = app.world().get::<DisplayPose>(source).unwrap().0.position;
        assert!((position.relative_to(GalacticPosition::ZERO).x - 50.).abs() < 1e-6);
        assert!((app.world().get::<OpticalLight>(source).unwrap().display_w - 20.).abs() < 1e-6);
        assert_eq!(
            app.world_mut()
                .query::<&Contact>()
                .iter(app.world())
                .count(),
            0
        );

        let mut third = snapshot(3, Id([2; 16]), Id([3; 16]), 0.);
        third.optical = vec![optical(2, 2000., 40.)];
        step(&mut app, 0.3, Some(third));
        assert!(app.world().get_entity(source).is_err());
        assert_eq!(
            app.world_mut()
                .query::<&Optical>()
                .iter(app.world())
                .count(),
            1
        );
    }

    #[test]
    fn optical_transit_instance_replaces_pose_and_light_samples() {
        let mut app = app();
        let mut first = snapshot(1, Id([2; 16]), Id([3; 16]), 0.);
        first.optical = vec![optical(1, 0., 10.)];
        step(&mut app, 0.1, Some(first));
        let old = app
            .world_mut()
            .query_filtered::<Entity, With<Optical>>()
            .single(app.world())
            .unwrap();
        let mut second = snapshot(2, Id([2; 16]), Id([3; 16]), 0.);
        let mut arrived = optical(1, 1e9, 1000.);
        arrived.spatial_instance = Id([12; 16]);
        second.optical.push(arrived);
        step(&mut app, 0.2, Some(second));
        step(&mut app, 0.25, None);
        assert!(app.world().get_entity(old).is_err());
        let (_, pose, light) = app
            .world_mut()
            .query::<(&Optical, &DisplayPose, &OpticalLight)>()
            .single(app.world())
            .unwrap();
        assert_eq!(pose.0.position.relative_to(GalacticPosition::ZERO).x, 1e9);
        assert_eq!(light.display_w, 1000.);
    }
}
