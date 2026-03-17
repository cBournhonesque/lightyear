use crate::stepper::{ClientServerStepper, StepperConfig};
use approx::assert_relative_eq;
use avian2d::math::Vector;
use avian2d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::frame_interpolation::FrameInterpolate;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::*;
use test_log::test;

/// The order is:
/// - RunFixedMainLoop: Transform -> GlobalTransform THEN GlobalTransform -> Position/Rotation
/// - PostUpdate: Position/Rotation -> Transform THEN Transform -> GlobalTransform
/// - PostUpdate: add Transform based on Position/Rotation THEN Transform -> GlobalTransforms
///
/// One thing to watch out for is that if FrameInterpolation is not enabled, the Position/Rotation
/// in FixedUpdate is not correct, since it gets updated via TransformToPosition.
/// FrameInterpolation restores the correct value of Position/Rotation before FixedUpdate.
///
/// if child colliders have a RigidBody, they are an independent RigidBody from the parent.
#[test]
fn test_replicate_position_child_rigidbody() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    // NOTE: for RigidBodies, we only use Position/Rotation
    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            RigidBody::Kinematic,
            Position::from_xy(1.0, 1.0),
            Rotation::default(),
            Collider::circle(1.0),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            RigidBody::Kinematic,
            Position::from_xy(3.0, 3.0),
            Rotation::default(),
        ))
        .id();
    info!(?server_parent, ?server_child, "Spawning entities on server");

    stepper.frame_step_server_first(1);

    // On server
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Position>(server_parent)
            .unwrap()
            .x,
        1.0
    );
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Position>(server_child)
            .unwrap()
            .x,
        3.0
    );
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Transform>(server_parent)
            .unwrap()
            .translation
            .x,
        1.0
    );
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Transform>(server_child)
            .unwrap()
            .translation
            .x,
        2.0
    );

    let client_parent = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_parent)
        .unwrap();
    let client_child = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_child)
        .unwrap();
    info!(?client_parent, ?client_child, "Received entities on client");
    assert_relative_eq!(
        stepper
            .client_app()
            .world()
            .get::<Position>(client_parent)
            .unwrap()
            .x,
        1.0
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<Position>(client_child)
            .unwrap()
            .x,
        3.0
    );
    assert_relative_eq!(
        stepper
            .client_app()
            .world()
            .get::<Transform>(client_parent)
            .unwrap()
            .translation
            .x,
        1.0
    );
    assert_relative_eq!(
        stepper
            .client_app()
            .world()
            .get::<Transform>(client_child)
            .unwrap()
            .translation
            .x,
        2.0
    );
}

/// Child colliders are child entities that have no RigidBody but have a collider.
/// They are 'part' of the parent's collider.
///
/// Their relative position must be indicated using Transform and NOT using Position/Rotation!
///
/// The child's world Position should be parent Position + child Transform offset.
///
/// BUG: With replicon, the child entity's GlobalTransform is corrupted on the first frame
/// because the parent doesn't have GlobalTransform when the child is spawned (B0004 warning).
/// This causes avian's GlobalTransformToPosition to compute an incorrect Position for the child,
/// and the incorrect value persists because avian overwrites the replicated Position every frame.
#[test]
#[ignore]
fn test_replicate_position_child_collider() {
    let mut config = StepperConfig::single();
    config.frame_duration = Duration::from_millis(5);
    let mut stepper = ClientServerStepper::from_config(config);

    // add colliders on the client to make sure that collider hierarchy systems work
    stepper.client_app().add_observer(
        |trigger: On<Add, Replicated>, q: Query<(), With<ChildOf>>, mut commands: Commands| {
            if let Ok(()) = q.get(trigger.entity) {
                commands
                    .entity(trigger.entity)
                    .insert(Collider::circle(1.0));
            } else {
                commands
                    .entity(trigger.entity)
                    .insert((Collider::circle(1.0), RigidBody::Kinematic));
            }
        },
    );

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            RigidBody::Kinematic,
            Position::from_xy(1.0, 1.0),
            // NOTE: Transform NEEDS to be present here, otherwise it's not added
            // on the parent before the child is spawned, and the child's Transform
            // becomes incorrect
            Transform::from_xyz(1.0, 1.0, 0.0),
            LinearVelocity(Vector::new(100.0, 0.0)),
            Collider::circle(1.0),
            Rotation::default(),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            // For child Colliders, we HAVE to use Transform to indicate the relative
            // positioning w.r.t the parent.
            Transform::from_xyz(2.0, 2.0, 0.0),
            Collider::circle(1.0),
        ))
        .id();
    info!(?server_parent, ?server_child, "Spawning entities on server");

    // Step enough frames for:
    // 1. replication to client
    // 2. client transform hierarchy propagation
    // 3. physics to advance
    stepper.frame_step_server_first(3);

    let client_parent = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_parent)
        .unwrap();
    let client_child = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_child)
        .unwrap();
    info!(?client_parent, ?client_child, "Received entities on client");

    // After several frames, the server and client should agree on positions.
    // The child's Transform is (2, 2, 0) relative to parent, so child Position.x = parent Position.x + 2.
    let server_parent_pos = stepper
        .server_app
        .world()
        .get::<Position>(server_parent)
        .unwrap()
        .x;
    let server_child_pos = stepper
        .server_app
        .world()
        .get::<Position>(server_child)
        .unwrap()
        .x;
    info!(server_parent_pos, server_child_pos, "Server positions");
    assert_relative_eq!(server_child_pos, server_parent_pos + 2.0, epsilon = 0.1);

    let client_parent_pos = stepper
        .client_app()
        .world()
        .get::<Position>(client_parent)
        .unwrap()
        .x;
    let client_child_pos = stepper
        .client_app()
        .world()
        .get::<Position>(client_child)
        .unwrap()
        .x;
    info!(client_parent_pos, client_child_pos, "Client positions");

    // Client positions should match server positions (within tolerance for replication delay)
    assert_relative_eq!(client_parent_pos, server_parent_pos, epsilon = 2.0);
    assert_relative_eq!(client_child_pos, server_child_pos, epsilon = 2.0);
    // The child offset from parent should be correct
    assert_relative_eq!(client_child_pos, client_parent_pos + 2.0, epsilon = 0.1);

    // Transform hierarchy should be correct
    let client_child_transform = stepper
        .client_app()
        .world()
        .get::<Transform>(client_child)
        .unwrap()
        .translation
        .x;
    assert_relative_eq!(client_child_transform, 2.0, epsilon = 0.1);

    let client_child_global = stepper
        .client_app()
        .world()
        .get::<GlobalTransform>(client_child)
        .unwrap()
        .compute_transform()
        .translation
        .x;
    assert_relative_eq!(client_child_global, client_child_pos, epsilon = 0.1);
}
