use crate::stepper::{ClientServerStepper, StepperConfig};
use approx::assert_relative_eq;
use avian2d::math::Vector;
use avian2d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::frame_interpolation::FrameInterpolate;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::{Replicate, ReplicationGroup};
use test_log::test;

/// The order is:
/// - RunFixedMainLoop: Transform -> GlobalTransform THEN GlobalTransform -> Position/Rotation
/// - PostUpdate: Position/Rotation -> Transform THEN Transform -> GlobalTransform
/// - PostUpdate: add Transform based on Position/Rotation THEN Transform -> GlobalTransforms
///
/// One thing to watch out for is that if FrameInterpolation is not enabled, the Position/Rotation
/// in FixedUpdate is not correct, since it gets updated via TransformToPosition.
/// FrameInterpolation restores the correct value of Position/Rotation before FixedUpdate.
#[test]
fn test_replicate_position() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            RigidBody::Dynamic,
            Position::from_xy(1.0, 1.0),
            Rotation::default(),
            ReplicationGroup::new_id(3),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            RigidBody::Dynamic,
            Position::from_xy(3.0, 3.0),
            Rotation::default(),
            ReplicationGroup::new_id(3),
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

#[ignore]
#[test]
fn test_replicate_position_movement() {
    let mut config = StepperConfig::single();
    config.frame_duration = Duration::from_millis(5);
    let mut stepper = ClientServerStepper::from_config(config);

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            RigidBody::Dynamic,
            Position::from_xy(1.0, 1.0),
            LinearVelocity(Vector::new(100.0, 0.0)),
            FrameInterpolate::<Position>::default(),
            Rotation::default(),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            RigidBody::Dynamic,
            Position::from_xy(3.0, 3.0),
            FrameInterpolate::<Position>::default(),
            Rotation::default(),
        ))
        .id();
    stepper
        .server_app
        .world_mut()
        .spawn(FixedJoint::new(server_parent, server_child));
    info!(?server_parent, ?server_child, "Spawning entities on server");

    stepper.frame_step_server_first(1);

    // On server, we didn't have time to run FixedUpdate yet
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
    // Transform has been updated via Position->Transform in PostUpdate
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

    stepper.frame_step_server_first(2);
    // This time we ran FixedUpdate once
    let frame_interp = stepper
        .server_app
        .world()
        .get::<FrameInterpolate<Position>>(server_parent);
    info!(?frame_interp, "parent");
    let frame_interp = stepper
        .server_app
        .world()
        .get::<FrameInterpolate<Position>>(server_child);
    info!(?frame_interp, "child");
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Position>(server_parent)
            .unwrap()
            .x,
        2.0
    );
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Position>(server_child)
            .unwrap()
            .x,
        4.0
    );
    assert_relative_eq!(
        stepper
            .server_app
            .world()
            .get::<Transform>(server_parent)
            .unwrap()
            .translation
            .x,
        2.0
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
}
