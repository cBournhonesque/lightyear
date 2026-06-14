use crate::stepper::{ClientServerStepper, StepperConfig};
use approx::assert_relative_eq;
use avian2d::math::Vector;
use avian2d::prelude::*;
use bevy::math::{Isometry2d, Rot2};
use bevy::prelude::*;
use bevy_replicon::prelude::Remote;
use core::time::Duration;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::frame_interpolation::FrameInterpolate;
use lightyear::prediction::prelude::{PredictionHistory, VisualCorrection};
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
    replicate_position_child_rigidbody(AvianReplicationMode::Position);
}

#[test]
fn test_position_but_interpolate_transform_child_rigidbody() {
    replicate_position_child_rigidbody(AvianReplicationMode::PositionButInterpolateTransform);
}

fn replicate_position_child_rigidbody(avian_mode: AvianReplicationMode) {
    let mut config = StepperConfig::single();
    config.avian_mode = avian_mode;
    let mut stepper = ClientServerStepper::from_config(config);

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
/// The test adds client-only physics components from PostUpdate after transform
/// propagation. Doing this from `Add<Remote>`/`Add<Replicated>` is
/// order-sensitive: even when `ChildOf` is received in the same replication
/// batch, Avian collider hooks run before the replicated hierarchy's
/// Transform/GlobalTransform state has been propagated.
#[test]
fn test_replicate_position_child_collider() {
    replicate_position_child_collider(AvianReplicationMode::Position);
}

#[test]
fn test_position_but_interpolate_transform_child_collider() {
    replicate_position_child_collider(AvianReplicationMode::PositionButInterpolateTransform);
}

#[test]
fn test_position_child_collider_visual_state_hierarchy() {
    child_collider_visual_state_hierarchy(AvianReplicationMode::Position);
}

#[test]
fn test_position_but_interpolate_transform_child_collider_visual_state_hierarchy() {
    child_collider_visual_state_hierarchy(AvianReplicationMode::PositionButInterpolateTransform);
}

#[test]
fn test_transform_child_collider_visual_state_hierarchy() {
    child_collider_visual_state_hierarchy(AvianReplicationMode::Transform);
}

fn replicate_position_child_collider(avian_mode: AvianReplicationMode) {
    let mut config = StepperConfig::single();
    config.avian_mode = avian_mode;
    config.frame_duration = Duration::from_millis(5);
    let mut stepper = ClientServerStepper::from_config(config);
    stepper.client_app().add_systems(
        PostUpdate,
        add_client_physics_after_transform_propagate.after(TransformSystems::Propagate),
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

    // Step enough for replication and hierarchy propagation.
    stepper.frame_step_server_first(2);

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

    // Step enough frames for:
    // 1. client collider hierarchy setup
    // 2. transform/collider-transform propagation
    // 3. physics to advance
    stepper.frame_step_server_first(3);

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

fn child_collider_visual_state_hierarchy(avian_mode: AvianReplicationMode) {
    let mut config = StepperConfig::single();
    config.avian_mode = avian_mode;
    config.frame_duration = Duration::from_millis(5);
    let mut stepper = ClientServerStepper::from_config(config);

    let parent = stepper
        .client_app()
        .world_mut()
        .spawn((
            RigidBody::Kinematic,
            Transform::from_xyz(1.0, 1.0, 0.0),
            Position::from_xy(1.0, 1.0),
            Rotation::default(),
            Collider::circle(1.0),
        ))
        .id();
    let child = stepper
        .client_app()
        .world_mut()
        .spawn((
            ChildOf(parent),
            Transform::from_xyz(2.0, 2.0, 0.0),
            Collider::circle(1.0),
        ))
        .id();

    stepper.frame_step_server_first(2);

    {
        let world = stepper.client_app().world_mut();
        assert!(world.get::<RigidBody>(child).is_none());
        assert!(world.get::<Collider>(child).is_some());
        insert_visual_state_components(world, child, avian_mode);
        mark_visual_state_changed(world, child, avian_mode);
        world.run_schedule(FixedPostUpdate);
        mark_visual_state_changed(world, child, avian_mode);
        world.run_schedule(FixedPostUpdate);
    }

    assert_prediction_history_respects_hierarchy(
        stepper.client_app().world(),
        parent,
        child,
        avian_mode,
    );
    assert_frame_interpolation_respects_hierarchy(
        stepper.client_app().world(),
        parent,
        child,
        avian_mode,
    );

    let before_frame_interpolation = child_global_x(stepper.client_app().world(), child);
    {
        let world = stepper.client_app().world_mut();
        set_frame_interpolation_visual_offset(world, child, avian_mode, 1.0);
        world.run_schedule(PostUpdate);
    }
    assert_child_hierarchy_consistent(stepper.client_app().world(), parent, child, avian_mode);
    assert!(
        child_global_x(stepper.client_app().world(), child) > before_frame_interpolation + 0.5,
        "frame interpolation should move the child visual while preserving hierarchy in {avian_mode:?}"
    );

    let before_visual_correction = child_global_x(stepper.client_app().world(), child);
    {
        let world = stepper.client_app().world_mut();
        hold_current_frame_interpolation_value(world, child, avian_mode);
        insert_visual_correction(world, child, avian_mode, 2.0);
        world.run_schedule(PostUpdate);
    }
    assert_child_hierarchy_consistent(stepper.client_app().world(), parent, child, avian_mode);
    assert!(
        child_global_x(stepper.client_app().world(), child) > before_visual_correction + 0.5,
        "visual correction should move the child visual while preserving hierarchy in {avian_mode:?}"
    );
}

fn insert_visual_state_components(
    world: &mut World,
    child: Entity,
    avian_mode: AvianReplicationMode,
) {
    match avian_mode {
        AvianReplicationMode::Position => {
            world.entity_mut(child).insert((
                PredictionHistory::<Position>::default(),
                PredictionHistory::<Rotation>::default(),
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
        AvianReplicationMode::PositionButInterpolateTransform => {
            world.entity_mut(child).insert((
                PredictionHistory::<Position>::default(),
                PredictionHistory::<Rotation>::default(),
                FrameInterpolate::<Transform>::default(),
            ));
        }
        AvianReplicationMode::Transform => {
            world.entity_mut(child).insert((
                PredictionHistory::<Transform>::default(),
                FrameInterpolate::<Transform>::default(),
            ));
        }
    }
}

fn mark_visual_state_changed(world: &mut World, child: Entity, avian_mode: AvianReplicationMode) {
    match avian_mode {
        AvianReplicationMode::Position | AvianReplicationMode::PositionButInterpolateTransform => {
            let position = *world.get::<Position>(child).unwrap();
            *world.get_mut::<Position>(child).unwrap() = position;
            let rotation = *world.get::<Rotation>(child).unwrap();
            *world.get_mut::<Rotation>(child).unwrap() = rotation;
        }
        AvianReplicationMode::Transform => {
            let transform = *world.get::<Transform>(child).unwrap();
            *world.get_mut::<Transform>(child).unwrap() = transform;
        }
    }
}

fn assert_prediction_history_respects_hierarchy(
    world: &World,
    parent: Entity,
    child: Entity,
    avian_mode: AvianReplicationMode,
) {
    match avian_mode {
        AvianReplicationMode::Position | AvianReplicationMode::PositionButInterpolateTransform => {
            let position = world.get::<Position>(child).unwrap();
            let history = world.get::<PredictionHistory<Position>>(child).unwrap();
            let history_position = history
                .most_recent()
                .and_then(|(_, state)| state.value())
                .unwrap();
            assert_relative_eq!(history_position.x, position.x, epsilon = 0.001);
            assert_relative_eq!(
                history_position.x,
                parent_global_x(world, parent) + child_local_x(world, child),
                epsilon = 0.001
            );
        }
        AvianReplicationMode::Transform => {
            let transform = world.get::<Transform>(child).unwrap();
            let history = world.get::<PredictionHistory<Transform>>(child).unwrap();
            let history_transform = history
                .most_recent()
                .and_then(|(_, state)| state.value())
                .unwrap();
            assert_relative_eq!(
                history_transform.translation.x,
                transform.translation.x,
                epsilon = 0.001
            );
        }
    }
}

fn assert_frame_interpolation_respects_hierarchy(
    world: &World,
    parent: Entity,
    child: Entity,
    avian_mode: AvianReplicationMode,
) {
    match avian_mode {
        AvianReplicationMode::Position => {
            let position = world.get::<Position>(child).unwrap();
            let frame_interpolate = world.get::<FrameInterpolate<Position>>(child).unwrap();
            let current = frame_interpolate.current_value.as_ref().unwrap();
            let previous = frame_interpolate.previous_value.as_ref().unwrap();
            assert_relative_eq!(current.x, position.x, epsilon = 0.001);
            assert_relative_eq!(previous.x, position.x, epsilon = 0.001);
            assert_relative_eq!(
                current.x,
                parent_global_x(world, parent) + child_local_x(world, child),
                epsilon = 0.001
            );
        }
        AvianReplicationMode::PositionButInterpolateTransform | AvianReplicationMode::Transform => {
            let transform = world.get::<Transform>(child).unwrap();
            let frame_interpolate = world.get::<FrameInterpolate<Transform>>(child).unwrap();
            let current = frame_interpolate.current_value.as_ref().unwrap();
            let previous = frame_interpolate.previous_value.as_ref().unwrap();
            assert_relative_eq!(
                current.translation.x,
                transform.translation.x,
                epsilon = 0.001
            );
            assert_relative_eq!(
                previous.translation.x,
                transform.translation.x,
                epsilon = 0.001
            );
            assert_relative_eq!(
                parent_global_x(world, parent) + current.translation.x,
                child_global_x(world, child),
                epsilon = 0.001
            );
        }
    }
}

fn set_frame_interpolation_visual_offset(
    world: &mut World,
    child: Entity,
    avian_mode: AvianReplicationMode,
    offset: f32,
) {
    match avian_mode {
        AvianReplicationMode::Position => {
            let mut visual = *world.get::<Position>(child).unwrap();
            visual.x += offset;
            let mut frame_interpolate = world.get_mut::<FrameInterpolate<Position>>(child).unwrap();
            frame_interpolate.previous_value = Some(visual);
            frame_interpolate.current_value = Some(visual);
        }
        AvianReplicationMode::PositionButInterpolateTransform | AvianReplicationMode::Transform => {
            let mut visual = *world.get::<Transform>(child).unwrap();
            visual.translation.x += offset;
            let mut frame_interpolate =
                world.get_mut::<FrameInterpolate<Transform>>(child).unwrap();
            frame_interpolate.previous_value = Some(visual);
            frame_interpolate.current_value = Some(visual);
        }
    }
}

fn hold_current_frame_interpolation_value(
    world: &mut World,
    child: Entity,
    avian_mode: AvianReplicationMode,
) {
    match avian_mode {
        AvianReplicationMode::Position => {
            let current = *world.get::<Position>(child).unwrap();
            let mut frame_interpolate = world.get_mut::<FrameInterpolate<Position>>(child).unwrap();
            frame_interpolate.previous_value = Some(current);
            frame_interpolate.current_value = Some(current);
        }
        AvianReplicationMode::PositionButInterpolateTransform | AvianReplicationMode::Transform => {
            let current = *world.get::<Transform>(child).unwrap();
            let mut frame_interpolate =
                world.get_mut::<FrameInterpolate<Transform>>(child).unwrap();
            frame_interpolate.previous_value = Some(current);
            frame_interpolate.current_value = Some(current);
        }
    }
}

fn insert_visual_correction(
    world: &mut World,
    child: Entity,
    avian_mode: AvianReplicationMode,
    offset: f32,
) {
    match avian_mode {
        AvianReplicationMode::Position => {
            world
                .entity_mut(child)
                .insert(VisualCorrection::<Position> {
                    error: Position::from_xy(offset, 0.0),
                });
        }
        AvianReplicationMode::PositionButInterpolateTransform | AvianReplicationMode::Transform => {
            world
                .entity_mut(child)
                .insert(VisualCorrection::<Isometry2d> {
                    error: Isometry2d::new(Vec2::new(offset, 0.0), Rot2::IDENTITY),
                });
        }
    }
}

fn assert_child_hierarchy_consistent(
    world: &World,
    parent: Entity,
    child: Entity,
    avian_mode: AvianReplicationMode,
) {
    assert_relative_eq!(
        child_global_x(world, child),
        parent_global_x(world, parent) + child_local_x(world, child),
        epsilon = 0.001
    );
    if avian_mode == AvianReplicationMode::Position {
        let position = world.get::<Position>(child).unwrap();
        assert_relative_eq!(child_global_x(world, child), position.x, epsilon = 0.001);
    }
}

fn parent_global_x(world: &World, parent: Entity) -> f32 {
    world
        .get::<GlobalTransform>(parent)
        .unwrap()
        .compute_transform()
        .translation
        .x
}

fn child_global_x(world: &World, child: Entity) -> f32 {
    world
        .get::<GlobalTransform>(child)
        .unwrap()
        .compute_transform()
        .translation
        .x
}

fn child_local_x(world: &World, child: Entity) -> f32 {
    world.get::<Transform>(child).unwrap().translation.x
}

#[derive(Component)]
struct ClientPhysicsAdded;

#[allow(clippy::type_complexity)]
fn add_client_physics_after_transform_propagate(
    mut commands: Commands,
    query: Query<
        (Entity, Option<&ChildOf>),
        (
            With<Remote>,
            With<Position>,
            With<Rotation>,
            With<Transform>,
            With<GlobalTransform>,
            Without<ClientPhysicsAdded>,
        ),
    >,
) {
    for (entity, child_of) in &query {
        if child_of.is_some() {
            commands
                .entity(entity)
                .insert((Collider::circle(1.0), ClientPhysicsAdded));
        } else {
            commands.entity(entity).insert((
                Collider::circle(1.0),
                RigidBody::Kinematic,
                ClientPhysicsAdded,
            ));
        }
    }
}
