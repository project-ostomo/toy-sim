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

pub fn apply(
    session: Res<GameSession>,
    mut commands: Commands,
    mut playback: ResMut<BufferedPlayback>,
    mut replication: ResMut<Replication>,
    mut clock: ResMut<RenderTime>,
    mut info: ResMut<SessionInfo>,
    mut navigation_state: ResMut<NavigationState>,
    mut playback_state: ResMut<PlaybackState>,
    mut feedback: ResMut<CommandState>,
    mut chat: ResMut<ChatState>,
    mut slip: ResMut<SlipEffects>,
    mut screens: ResMut<ScreenFrames>,
    old_samples: Query<(&SpatialInstance, &PoseSamples, Option<&VisualSamples>)>,
    old_optical: Query<&Optical>,
    old_owned: Query<&OwnedShip>,
) {
    let playback = &mut playback.0;
    if playback.world != Some(session.key.world) {
        return;
    }
    let advanced = if replication.applied != Some(session.key) {
        playback.apply_first().is_some()
    } else {
        playback.tick().is_some()
    };
    playback_state.target_frames = playback.target_frames;
    playback_state.underruns = playback.underruns;
    if !advanced {
        return;
    }
    let sequence = playback.frame().unwrap().sequence;
    let publications = playback.take_publications(sequence);
    for update in &publications.screens {
        screens.0.insert((update.ship, update.slot), update.clone());
    }
    let frame = playback.frame().unwrap();
    replication.applied = Some(session.key);
    clock.previous_ns = if info.sequence == 0 {
        frame.sim_time_ns
    } else {
        clock.current_ns
    };
    clock.current_ns = frame.sim_time_ns;
    slip.0 = frame.presentation.slip.clone();
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
    playback_state.diagnostics = frame.presentation.diagnostics.clone();
    let catalogue = frame.presentation.navigation.directory;
    if navigation_state.navigation.beacons != frame.presentation.navigation.beacons {
        let navigation = std::sync::Arc::make_mut(&mut navigation_state.navigation);
        if navigation
            .beacons
            .iter()
            .map(|beacon| (beacon.id, &beacon.systems))
            .ne(frame
                .presentation
                .navigation
                .beacons
                .iter()
                .map(|beacon| (beacon.id, &beacon.systems)))
        {
            navigation.topology_revision = navigation.topology_revision.wrapping_add(1);
        }
        navigation.beacons = frame.presentation.navigation.beacons.clone();
    }
    if navigation_state.navigation_hash != catalogue {
        navigation_state.navigation_hash = catalogue;
        if catalogue.is_none() {
            let owned: Vec<_> = navigation_state
                .inhabited
                .ownership
                .keys()
                .copied()
                .collect();
            {
                let universe = &session.universe;
                let navigation = std::sync::Arc::make_mut(&mut navigation_state.navigation);
                for id in owned {
                    if let Some(index) = universe.system_index(id.0) {
                        if let Some(system) = navigation.systems.get_mut(index) {
                            system.sovereignty = None;
                        }
                    }
                }
                navigation.topology_revision = navigation.topology_revision.wrapping_add(1);
            }
            navigation_state.inhabited = Default::default();
        }
        navigation_state.navigation_status = if catalogue.is_some() {
            NavigationStatus::Loading
        } else {
            NavigationStatus::Unavailable
        };
    }
    for update in publications.chat {
        chat.apply(update);
    }
    feedback.events.extend(publications.events);
    let excess = feedback.events.len().saturating_sub(128);
    feedback.events.drain(..excess);
    feedback.results.extend(publications.results);
    let excess = feedback.results.len().saturating_sub(128);
    feedback.results.drain(..excess);

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
    for (group, tracks) in &frame.contacts {
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
                observer: *group,
                contact: track.id,
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
            // Slip changes optical identity, but the owned ship's path is continuous.
            let continuous_slip = old_owned.get(entity).is_ok_and(|previous| {
                matches!(
                    (&previous.0.presence, &ship.presence),
                    (
                        osg_model::travel::Presence::Space,
                        osg_model::travel::Presence::SlipTransit(_)
                    ) | (
                        osg_model::travel::Presence::SlipTransit(_),
                        osg_model::travel::Presence::Space
                    )
                )
            });
            let old = old_samples
                .get(entity)
                .ok()
                .filter(|(instance, _, _)| instance.0 == ship.spatial_instance || continuous_slip)
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
        commands
            .entity(entity)
            .insert(ViewObservation(view.clone()));
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
    fn replication_preserves_locally_selected_systems() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = 3_u64;
        let mut first = snapshot(1, group, track, 0.);
        first.views.push(ViewState {
            id: 41,
            revision: 1,
            focused_ship: None,
            origin: GalacticPosition::ZERO,
        });
        step(&mut app, 0.1, Some(first.clone()));
        let mut views = app
            .world_mut()
            .query_filtered::<Entity, With<ViewObservation>>();
        let view = views.single(app.world()).unwrap();
        let system = Id([8; 16]);
        app.world_mut()
            .entity_mut(view)
            .insert(ViewSystems(vec![system]));

        first.tick = 2;
        first.sequence = 2;
        first.sim_time_ns = 200_000_000;
        step(&mut app, 0.2, Some(first));
        assert_eq!(app.world().get::<ViewSystems>(view).unwrap().0, [system]);
    }

    #[test]
    fn live_beacons_replicate_independently_and_catalogue_change_clears_old_asset() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = 3_u64;
        let beacon = NavigationBeacon {
            id: Id([4; 16]),
            systems: vec![Id([5; 16])],
            name: "Live station".into(),
            pose: Pose::default(),
            radius_m: 100.,
            navigation: false,
            docking: true,
        };
        let mut first = snapshot(1, group, track, 0.);
        first.presentation.navigation = std::sync::Arc::new(NavigationSnapshot {
            directory: Some([6; 32]),
            beacons: vec![beacon.clone()],
        });
        step(&mut app, 0.1, Some(first.clone()));
        assert_eq!(
            app.world().resource::<NavigationState>().navigation_status,
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
        app.world_mut().resource_mut::<NavigationState>().navigation = installed.clone();
        let mut next = snapshot(2, group, track, 0.);
        next.presentation.navigation = first.presentation.navigation;
        step(&mut app, 0.2, Some(next));
        assert!(std::sync::Arc::ptr_eq(
            &installed,
            &app.world().resource::<NavigationState>().navigation
        ));

        let mut replacement = snapshot(3, group, track, 0.);
        replacement.presentation.navigation = std::sync::Arc::new(NavigationSnapshot {
            directory: Some([7; 32]),
            beacons: Vec::new(),
        });
        step(&mut app, 0.3, Some(replacement));
        assert!(
            app.world()
                .resource::<NavigationState>()
                .navigation
                .beacons
                .is_empty()
        );
        assert_eq!(
            app.world().resource::<NavigationState>().navigation_status,
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

    fn snapshot(sequence: u64, group: Id, track: u64, position: f64) -> Frame {
        let mut frame = Frame {
            chat: None,
            optical: Vec::new(),
            calendar_unix_ms: 0,
            presentation: PresentationFrame::default(),
            world: Id([1; 16]),
            sequence,
            tick: sequence,
            sim_time_ns: sequence * osg_model::TICK_NS,
            rate: 1.,
            views: Vec::new(),
            contacts: BTreeMap::new(),
            ships: Vec::new(),
            screens: Vec::new(),
            events: Vec::new(),
            results: Vec::new(),
        };
        frame.contacts.insert(
            group,
            vec![SensorObservation {
                id: track,
                entity: None,
                spatial_instance: Id([5; 16]),
                pose: Pose {
                    position: GalacticPosition::ZERO.offset_by(glam::DVec3::X * position),
                    ..default()
                },
                iff: None,
                radius_m: 1.,
            }],
        );
        frame
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_resource(Time::<Fixed>::from_duration(osg_model::TICK_DURATION))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
            .init_resource::<Replication>()
            .init_resource::<RenderTime>()
            .init_resource::<SlipEffects>()
            .init_resource::<SessionInfo>()
            .init_resource::<Bootstrap>()
            .init_resource::<SessionEvents>()
            .init_resource::<Outgoing>()
            .init_resource::<NavigationState>()
            .init_resource::<PlaybackState>()
            .init_resource::<ScreenFrames>()
            .add_observer(reset_resource::<ScreenFrames>)
            .init_resource::<CommandState>()
            .init_resource::<ChatState>()
            .add_observer(reset_resource::<NavigationState>)
            .add_observer(reset_resource::<PlaybackState>)
            .add_observer(reset_resource::<CommandState>)
            .add_observer(reset_resource::<ChatState>)
            .insert_resource(BufferedPlayback(Playback::new(true)))
            .add_systems(Update, interpolate.in_set(ClientSystems::Gameplay));
        lifecycle::install(&mut app);
        app.update();
        app
    }

    fn step(app: &mut App, seconds: f64, frame: Option<Frame>) {
        let elapsed = app.world().resource::<Time<Real>>().elapsed();
        let delta = Duration::from_secs_f64(seconds) - elapsed;
        if let Some(frame) = frame {
            if app
                .world()
                .resource::<Bootstrap>()
                .key
                .is_none_or(|key| key.world != frame.world)
            {
                app.world_mut()
                    .resource_mut::<Bootstrap>()
                    .prepared(frame.world);
                app.world_mut()
                    .resource_mut::<NextState<ClientPhase>>()
                    .set(ClientPhase::Loading);
                app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
                app.update();
            }
            app.world_mut()
                .resource_mut::<BufferedPlayback>()
                .0
                .receive(frame);
        }
        app.insert_resource(TimeUpdateStrategy::ManualDuration(delta));
        app.update();
        // Activation follows replication at the next normal state transition.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        app.update();
    }

    #[test]
    fn screen_publications_update_and_clear_with_the_session() {
        let mut app = app();
        let ship = Id([4; 16]);
        let group = Id([2; 16]);
        let mut frame = snapshot(1, group, 3, 0.);
        frame.screens.push(ScreenUpdate {
            ship,
            slot: 0,
            revision: 7,
            tick: 1,
            frame: Some(drawing::ScreenImage {
                screen_id: 0,
                background: [0; 3],
                width: 512,
                height: 256,
                draws: Vec::new(),
                buttons: Default::default(),
            }),
            error: None,
        });
        step(&mut app, 0.1, Some(frame));
        assert!(
            app.world().resource::<ScreenFrames>().0[&(ship, 0)]
                .frame
                .is_some()
        );

        let mut frame = snapshot(2, group, 3, 0.);
        frame.screens.push(ScreenUpdate {
            ship,
            slot: 0,
            revision: 8,
            tick: 2,
            frame: None,
            error: Some("Display unavailable".into()),
        });
        step(&mut app, 0.2, Some(frame));
        assert!(
            app.world().resource::<ScreenFrames>().0[&(ship, 0)]
                .frame
                .is_none()
        );

        let mut frame = snapshot(1, group, 3, 0.);
        frame.world = Id([9; 16]);
        step(&mut app, 0.3, Some(frame));
        assert!(app.world().resource::<ScreenFrames>().0.is_empty());
    }

    #[test]
    fn catchup_delivers_every_chat_page_in_sequence() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = 3_u64;
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let mut outgoing = Outgoing::default();
        app.world_mut().resource_mut::<ChatState>().subscribe(
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
                frame.chat = Some(osg_model::chat::ChatUpdate {
                    subscription_revision: 1,
                    view: 1,
                    view_revision: 7,
                    unavailable: false,
                    page: osg_model::chat::ChatPage {
                        messages: vec![osg_model::chat::ChatMessage {
                            id: Id([sequence as u8; 16]),
                            sequence,
                            tick: sequence,
                            calendar_unix_ms: osg_model::calendar::FOUR_HUNDRED_YEARS_MS,
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
                .receive(frame);
        }
        step(&mut app, 0.2, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 3);
        let chat = app.world().resource::<ChatState>();
        assert_eq!(
            chat.messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["Message 2", "Message 3"]
        );
    }

    #[test]
    fn contacts_are_stable_observer_scoped_entities_with_interpolated_components() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = 3_u64;
        let mut first = snapshot(1, group, track, 0.);
        first.contacts.get_mut(&group).unwrap()[0].pose.velocity[0] = 10.;
        first.contacts.get_mut(&group).unwrap()[0]
            .pose
            .angular_velocity[0] = 0.2;
        step(&mut app, 0.1, Some(first));
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<Contact>>()
            .single(app.world())
            .unwrap();
        let mut second = snapshot(2, group, track, 100.);
        second.contacts.get_mut(&group).unwrap()[0].pose.velocity[0] = 30.;
        second.contacts.get_mut(&group).unwrap()[0]
            .pose
            .angular_velocity[0] = 0.6;
        let other_group = Id([4; 16]);
        second
            .contacts
            .insert(other_group, second.contacts[&group].clone());
        step(&mut app, 0.2, Some(second));
        step(&mut app, 0.25, None);
        assert_eq!(
            app.world_mut()
                .query::<&Contact>()
                .iter(app.world())
                .count(),
            2
        );
        assert_eq!(
            app.world().get::<Contact>(entity).unwrap().1.observer,
            group
        );
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
        let track = 3_u64;
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<Contact>>()
            .single(app.world())
            .unwrap();
        let mut next = snapshot(2, group, track, 1000.);
        next.contacts.get_mut(&group).unwrap()[0].spatial_instance = Id([6; 16]);
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
        let track = 3_u64;
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        let mut next = snapshot(2, group, track, 100.);
        next.contacts.clear();
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
    fn owned_ship(ship: Id) -> ShipTelemetry {
        ShipTelemetry {
            location: Default::default(),
            can_control: true,
            appearance: None,
            radius_m: 10.,
            dock_services: Default::default(),
            iff: IffIdentity {
                owner: Id([1; 16]),
                faction: None,
                labels: default(),
                enabled: true,
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
        }
    }

    #[test]
    fn owned_slip_boundaries_interpolate_without_bridging_teleports() {
        let mut app = app();
        let id = Id([4; 16]);
        for (sequence, presence, x, expected) in [
            (1, travel::Presence::Space, 0.0, 0.0),
            (2, travel::Presence::SlipTransit(Id([9; 16])), 100.0, 50.0),
            (3, travel::Presence::Space, 200.0, 150.0),
            (4, travel::Presence::Space, 10000.0, 10000.0),
        ] {
            let mut frame = snapshot(sequence, Id([2; 16]), 3, 0.0);
            let mut ship = owned_ship(id);
            ship.presence = presence;
            ship.spatial_instance = Id([sequence as u8; 16]);
            ship.pose.as_mut().unwrap().position =
                GalacticPosition::ZERO.offset_by(glam::DVec3::X * x);
            frame.ships.push(ship);
            step(
                &mut app,
                sequence as f64 * osg_model::TICK_SECONDS,
                Some(frame),
            );
            step(
                &mut app,
                sequence as f64 * osg_model::TICK_SECONDS + 0.05,
                None,
            );
            let world = app.world_mut();
            let pose = world
                .query_filtered::<&DisplayPose, With<OwnedShip>>()
                .single(world)
                .unwrap();
            assert!(
                (pose.0.position.relative_to(GalacticPosition::ZERO).x - expected).abs() < 1e-6
            );
        }
    }

    #[test]
    fn docking_removes_space_pose_and_capture_removes_private_telemetry() {
        let mut app = app();
        let mut first = snapshot(1, Id([2; 16]), 3, 0.);
        let ship = Id([4; 16]);
        first.ships.push(owned_ship(ship));
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
        let track = 3_u64;
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
        assert_eq!(app.world().resource::<PlaybackState>().underruns, 1);
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
        let track = 3_u64;
        step(&mut app, 0.05, Some(snapshot(1, group, track, 0.)));
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 0);
        step(&mut app, 0.1, None);
        assert_eq!(
            app.world().resource::<RenderTime>().display_ns,
            osg_model::TICK_NS
        );

        let mut accelerated = snapshot(2, group, track, 100.);
        accelerated.sim_time_ns = 1_100_000_000;
        accelerated.tick = 11;
        accelerated.rate = 10.;
        step(&mut app, 0.2, Some(accelerated.clone()));
        step(&mut app, 0.25, None);
        assert_eq!(app.world().resource::<RenderTime>().display_ns, 600_000_000);

        let mut normal = accelerated.clone();
        normal.sequence = 3;
        normal.tick = 12;
        normal.sim_time_ns = 1_200_000_000;
        normal.rate = 1.;
        step(&mut app, 0.3, Some(normal.clone()));
        step(&mut app, 0.35, None);
        assert_eq!(
            app.world().resource::<RenderTime>().display_ns,
            1_150_000_000
        );
        normal.sequence = 4;
        normal.tick = 13;
        normal.sim_time_ns = 1_300_000_000;
        app.world_mut()
            .resource_mut::<BufferedPlayback>()
            .0
            .receive(normal.clone());
        normal.sequence = 5;
        normal.tick = 14;
        normal.sim_time_ns = 1_400_000_000;
        app.world_mut()
            .resource_mut::<BufferedPlayback>()
            .0
            .receive(normal);
        step(&mut app, 0.55, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 5);
    }

    #[test]
    fn catchup_preserves_both_publications_and_interpolates_to_the_final_sample() {
        let mut app = app();
        let group = Id([2; 16]);
        let track = 3_u64;
        step(&mut app, 0.1, Some(snapshot(1, group, track, 0.)));
        for sequence in 2..=14 {
            let mut frame = snapshot(sequence, group, track, (sequence - 1) as f64 * 100.);
            if sequence <= 3 {
                frame.results.push(CommandResult {
                    id: Id((sequence as u128).to_le_bytes()),
                    error: Some("test result".into()),
                });
                frame.events.push(osg_model::Event {
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
                        target: Id([3; 16]),
                        pose: Pose::default(),
                        appearance: None,
                        energy_j: 1.,
                        mass_kg: 1.,
                        radius_m: 1.,
                    },
                });
                frame.contacts.get_mut(&group).unwrap()[0].spatial_instance = Id([6; 16]);
            }
            app.world_mut()
                .resource_mut::<BufferedPlayback>()
                .0
                .receive(frame);
        }
        step(&mut app, 0.2, None);
        assert_eq!(app.world().resource::<SessionInfo>().sequence, 3);
        let session = app.world().resource::<CommandState>();
        assert_eq!(
            session
                .results
                .iter()
                .map(|r| u128::from_le_bytes(r.id.0))
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
        assert_eq!(app.world().resource::<CommandState>().results.len(), 2);
    }
    fn optical(view: u64, position: f64, luminosity_w: f64) -> optical::OpticalObservation {
        optical::OpticalObservation {
            view,
            id: Id([9; 16]),
            spatial_instance: Id([10; 16]),
            iff: None,
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
                slip_readiness: 0.0,
                engines: Vec::new(),
                turrets: Vec::new(),
                shield: None,
            },
        }
    }

    #[test]
    fn optical_entities_are_view_scoped_independent_of_radio_tracks_and_interpolate_light() {
        let mut app = app();
        let mut first = snapshot(1, Id([2; 16]), 3, 0.);
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

        let mut second = snapshot(2, Id([2; 16]), 3, 0.);
        second.contacts.clear();
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

        let mut third = snapshot(3, Id([2; 16]), 3, 0.);
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
        let mut first = snapshot(1, Id([2; 16]), 3, 0.);
        first.optical = vec![optical(1, 0., 10.)];
        step(&mut app, 0.1, Some(first));
        let old = app
            .world_mut()
            .query_filtered::<Entity, With<Optical>>()
            .single(app.world())
            .unwrap();
        let mut second = snapshot(2, Id([2; 16]), 3, 0.);
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
