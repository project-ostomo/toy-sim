use super::*;

fn world() -> World {
    let mut world = World::new();
    world.init_resource::<IdentityIndex>();
    world
}

#[test]
fn replacing_identity_removes_previous_alias() {
    let mut world = world();
    let original = Id::new();
    let replacement = Id::new();
    let entity = world.spawn(Identity(original)).id();

    register(&mut world, entity, replacement).unwrap();

    assert!(lookup(&world, original).is_err());
    assert_eq!(lookup(&world, replacement).unwrap(), entity);
    assert_eq!(world.resource::<IdentityIndex>().entries().len(), 1);

    // Direct ECS replacement follows the same lifecycle as registration.
    world.entity_mut(entity).insert(Identity(original));

    assert!(lookup(&world, replacement).is_err());
    assert_eq!(lookup(&world, original).unwrap(), entity);
}

#[test]
fn removal_and_despawn_release_identity_immediately() {
    let mut world = world();
    let id = Id::new();
    let original = world.spawn(Identity(id)).id();

    world.entity_mut(original).remove::<Identity>();
    assert!(lookup(&world, id).is_err());
    assert!(world.resource::<IdentityIndex>().entries().is_empty());

    let replacement = world.spawn_empty().id();
    register(&mut world, replacement, id).unwrap();
    assert_eq!(lookup(&world, id).unwrap(), replacement);

    world.despawn(replacement);
    assert!(lookup(&world, id).is_err());
    assert!(world.resource::<IdentityIndex>().entries().is_empty());
}

#[test]
fn duplicate_registration_preserves_both_existing_identities() {
    let mut world = world();
    let first_id = Id::new();
    let second_id = Id::new();
    let first = world.spawn(Identity(first_id)).id();
    let second = world.spawn(Identity(second_id)).id();

    assert!(register(&mut world, second, first_id).is_err());

    assert_eq!(world.get::<Identity>(first).unwrap().0, first_id);
    assert_eq!(world.get::<Identity>(second).unwrap().0, second_id);
    assert_eq!(lookup(&world, first_id).unwrap(), first);
    assert_eq!(lookup(&world, second_id).unwrap(), second);
    assert_eq!(world.resource::<IdentityIndex>().entries().len(), 2);

    register(&mut world, first, first_id).unwrap();
    assert_eq!(lookup(&world, first_id).unwrap(), first);
}
