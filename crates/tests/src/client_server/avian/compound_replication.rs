use crate::stepper::{ClientServerStepper, StepperConfig};
use approx::assert_relative_eq;
use avian2d::collision::collider::EnlargedAabb;
use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::*;
use test_log::test;

const ROOT_POSITION: Vec2 = Vec2::new(5.0, -3.0);
const ROOT_ANGLE: f32 = 0.4;

#[test]
fn compound_hierarchy_position_mode() {
    run_compound_hierarchy(AvianReplicationMode::Position);
}

#[test]
fn compound_hierarchy_position_interpolate_transform_mode() {
    run_compound_hierarchy(AvianReplicationMode::PositionButInterpolateTransform);
}

#[test]
fn compound_hierarchy_transform_mode() {
    run_compound_hierarchy(AvianReplicationMode::Transform);
}

fn run_compound_hierarchy(mode: AvianReplicationMode) {
    let mut config = StepperConfig::single();
    config.avian_mode = mode;
    let mut stepper = ClientServerStepper::from_config(config);

    let root_bundle = (
        Replicate::to_clients(NetworkTarget::All),
        RigidBody::Kinematic,
        Position::from(ROOT_POSITION),
        Rotation::radians(ROOT_ANGLE),
        Transform::from_xyz(ROOT_POSITION.x, ROOT_POSITION.y, 0.0)
            .with_rotation(Quat::from_rotation_z(ROOT_ANGLE)),
    );
    let root = stepper.server_app.world_mut().spawn(root_bundle).id();

    let direct = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(root),
            Transform::from_xyz(-2.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(-0.2)),
            Collider::rectangle(1.5, 0.75),
            CollisionLayers::default(),
        ))
        .id();
    let pivot = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(root),
            Transform::from_xyz(4.0, -1.0, 0.0).with_rotation(Quat::from_rotation_z(0.3)),
        ))
        .id();
    let nested = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(pivot),
            Transform::from_xyz(3.0, 2.0, 0.0).with_rotation(Quat::from_rotation_z(0.15)),
            Collider::circle(0.8),
            CollisionLayers::from_bits(0b0010, LayerMask::ALL.0),
        ))
        .id();
    let sensor = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(root),
            Transform::from_xyz(0.0, 5.0, 0.0),
            Collider::circle(1.25),
            Sensor,
            CollisionEventsEnabled,
            CollisionLayers::from_bits(0b0100, LayerMask::ALL.0),
        ))
        .id();
    let child_body = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(root),
            RigidBody::Kinematic,
            Position::from_xy(-1.0, -4.0),
            Rotation::radians(-0.1),
            Transform::from_xyz(-6.0, 0.0, 0.0).with_rotation(Quat::from_rotation_z(-0.5)),
            Collider::circle(0.6),
        ))
        .id();

    stepper.frame_step_server_first(2);

    let mapper = &stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper;
    let client_root = mapper.get_local(root).expect("root was not replicated");
    let client_direct = mapper
        .get_local(direct)
        .expect("direct child was not replicated");
    let client_pivot = mapper.get_local(pivot).expect("pivot was not replicated");
    let client_nested = mapper
        .get_local(nested)
        .expect("nested child was not replicated");
    let client_sensor = mapper.get_local(sensor).expect("sensor was not replicated");
    let client_child_body = mapper
        .get_local(child_body)
        .expect("child rigid body was not replicated");

    // The example-style blueprint path reconstructs local-only physics on the
    // remote hierarchy. Insert ColliderOf explicitly so the nested collider is
    // attached correctly even before Avian has observed every deferred parent
    // transform in the replicated hierarchy.
    let client_world = stepper.client_app().world_mut();
    client_world
        .entity_mut(client_root)
        .insert(RigidBody::Kinematic);
    client_world.entity_mut(client_direct).insert((
        Transform::from_xyz(-2.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(-0.2)),
        Collider::rectangle(1.5, 0.75),
        ColliderOf { body: client_root },
        CollisionLayers::default(),
    ));
    client_world
        .entity_mut(client_pivot)
        .insert(Transform::from_xyz(4.0, -1.0, 0.0).with_rotation(Quat::from_rotation_z(0.3)));
    client_world.entity_mut(client_nested).insert((
        Transform::from_xyz(3.0, 2.0, 0.0).with_rotation(Quat::from_rotation_z(0.15)),
        Collider::circle(0.8),
        ColliderOf { body: client_root },
        CollisionLayers::from_bits(0b0010, LayerMask::ALL.0),
    ));
    client_world.entity_mut(client_sensor).insert((
        Transform::from_xyz(0.0, 5.0, 0.0),
        Collider::circle(1.25),
        ColliderOf { body: client_root },
        Sensor,
        CollisionEventsEnabled,
        CollisionLayers::from_bits(0b0100, LayerMask::ALL.0),
    ));
    client_world.entity_mut(client_child_body).insert((
        Transform::from_xyz(-6.0, 0.0, 0.0).with_rotation(Quat::from_rotation_z(-0.5)),
        RigidBody::Kinematic,
        Collider::circle(0.6),
    ));

    stepper.frame_step_server_first(4);

    assert_compound_state(
        stepper.server_app.world(),
        root,
        direct,
        pivot,
        nested,
        sensor,
        child_body,
        "server",
    );
    assert_compound_state(
        stepper.client_app().world(),
        client_root,
        client_direct,
        client_pivot,
        client_nested,
        client_sensor,
        client_child_body,
        "client",
    );

    let server_root_position = *stepper.server_app.world().get::<Position>(root).unwrap();
    let client_root_position = *stepper
        .client_app()
        .world()
        .get::<Position>(client_root)
        .unwrap();
    assert_relative_eq!(
        client_root_position.0,
        server_root_position.0,
        epsilon = 0.001
    );
}

#[allow(clippy::too_many_arguments)]
fn assert_compound_state(
    world: &World,
    root: Entity,
    direct: Entity,
    pivot: Entity,
    nested: Entity,
    sensor: Entity,
    child_body: Entity,
    label: &str,
) {
    assert!(
        world.get::<RigidBody>(root).is_some(),
        "{label}: root rigid body missing"
    );
    assert!(
        world.get::<Collider>(root).is_none(),
        "{label}: root must not have a collider"
    );

    assert_eq!(world.get::<ChildOf>(direct).unwrap().parent(), root);
    assert_eq!(world.get::<ChildOf>(pivot).unwrap().parent(), root);
    assert_eq!(world.get::<ChildOf>(nested).unwrap().parent(), pivot);
    assert_eq!(world.get::<ChildOf>(sensor).unwrap().parent(), root);
    assert_eq!(world.get::<ChildOf>(child_body).unwrap().parent(), root);

    assert!(
        world.get::<Collider>(pivot).is_none(),
        "{label}: pivot must be transform-only"
    );
    assert!(
        world.get::<RigidBody>(pivot).is_none(),
        "{label}: pivot became a rigid body"
    );

    for collider in [direct, nested, sensor] {
        assert_eq!(
            world
                .get::<ColliderOf>(collider)
                .map(|collider_of| collider_of.body),
            Some(root),
            "{label}: compound collider {collider:?} has the wrong body"
        );
        assert!(world.get::<ColliderTransform>(collider).is_some());
        assert!(world.get::<ColliderAabb>(collider).is_some());
        assert!(world.get::<EnlargedAabb>(collider).is_some());
        assert!(world.get::<Position>(collider).is_some());
        assert!(world.get::<GlobalTransform>(collider).is_some());
    }

    assert!(
        world.get::<Sensor>(sensor).is_some(),
        "{label}: sensor marker missing"
    );
    assert_eq!(
        world.get::<CollisionLayers>(nested).unwrap().memberships,
        LayerMask(0b0010)
    );
    assert_eq!(
        world.get::<CollisionLayers>(sensor).unwrap().memberships,
        LayerMask(0b0100)
    );

    assert!(world.get::<RigidBody>(child_body).is_some());
    assert_eq!(
        world
            .get::<ColliderOf>(child_body)
            .map(|collider_of| collider_of.body),
        Some(child_body),
        "{label}: child rigid body collider must attach to itself"
    );
}
