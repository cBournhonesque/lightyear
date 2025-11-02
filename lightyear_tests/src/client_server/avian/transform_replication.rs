use crate::stepper::{ClientServerStepper, StepperConfig};
use approx::assert_relative_eq;
use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::avian2d::plugin::AvianReplicationMode;
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
    let mut config = StepperConfig::single();
    config.avian_mode = AvianReplicationMode::Transform;
    let mut stepper = ClientServerStepper::from_config(config);

    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            RigidBody::Dynamic,
            Transform::from_xyz(1.0, 0.0, 0.0),
            ReplicationGroup::new_id(3),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            ChildOf(server_parent),
            RigidBody::Dynamic,
            Transform::from_xyz(2.0, 0.0, 0.0),
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
