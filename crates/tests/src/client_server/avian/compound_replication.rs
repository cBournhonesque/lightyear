use crate::protocol::CompA;
use crate::stepper::{ClientServerStepper, StepperConfig};
use approx::assert_relative_eq;
use avian2d::collision::collider::EnlargedAabb;
use avian2d::physics_transform::ApplyPosToTransform;
use avian2d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationHistory};
use lightyear::prediction::correction::VisualCorrection;
use lightyear::prediction::diagnostics::PredictionMetrics;
use lightyear::prediction::rollback::RollbackSystems;
use lightyear::prelude::{ConfirmedHistory, Interpolated, Predicted};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::*;
use test_log::test;

const ROOT_POSITION: Vec2 = Vec2::new(5.0, -3.0);
const ROOT_ANGLE: f32 = 0.4;
const EXAMPLE_PLAYER_SIZE: f32 = 40.0;
const EXAMPLE_CHILD_SIZE: f32 = 16.0;
const EXAMPLE_CHILD_OFFSET: Vec2 = Vec2::new((EXAMPLE_PLAYER_SIZE + EXAMPLE_CHILD_SIZE) / 2.0, 0.0);

/// Test-local marker for the deterministic child in the compound-player template.
#[derive(Component)]
struct CompoundChildCollider;

#[test]
fn compound_hierarchy_position_mode() {
    run_compound_hierarchy(AvianReplicationMode::Position {
        sync_to_transform: false,
    });
}

#[test]
fn compound_hierarchy_transform_mode() {
    run_compound_hierarchy(AvianReplicationMode::Transform);
}

/// Exercise the example's intended compound-player contract through the complete
/// client visual pipeline. The owner's root is predicted and frame-interpolated;
/// the other client's root is snapshot-interpolated. Each client constructs the
/// deterministic child locally; it is timeline-neutral and follows whichever root
/// representation is present. A forced misprediction must trigger
/// rollback/correction without opening a gap between the two colliders.
#[test]
fn touching_child_collider_survives_prediction_interpolation_and_correction() {
    let mut config = StepperConfig::with_netcode_clients(2);
    config.frame_duration = Duration::from_millis(5);
    let mut stepper = ClientServerStepper::from_config(config);

    for client in &mut stepper.client_apps {
        client.init_resource::<TouchAudit>();
        client.init_resource::<CorrectionObserved>();
        client.init_resource::<InjectMisprediction>();
        client.add_systems(FixedUpdate, inject_fixed_misprediction);
        client.add_systems(
            PostUpdate,
            materialize_example_compound_player.after(TransformSystems::Propagate),
        );
        client.add_systems(
            PostUpdate,
            record_touching_faces.after(TransformSystems::Propagate),
        );
        client.add_systems(
            PostUpdate,
            record_visual_correction.after(RollbackSystems::VisualCorrection),
        );
    }

    let owner = stepper
        .client_of(0)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;
    let root = stepper
        .server_app
        .world_mut()
        .spawn((
            CompA(1.0),
            Replicate::to_clients(NetworkTarget::All),
            DisableReplicateHierarchy,
            PredictionTarget::to_clients(NetworkTarget::Single(owner)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(owner)),
            RigidBody::Kinematic,
            Position::from(ROOT_POSITION),
            Rotation::radians(ROOT_ANGLE),
            Transform::from_translation(ROOT_POSITION.extend(0.0))
                .with_rotation(Quat::from_rotation_z(ROOT_ANGLE)),
            LinearVelocity(Vec2::new(20.0, 4.0)),
            AngularVelocity(0.4),
            Collider::rectangle(EXAMPLE_PLAYER_SIZE, EXAMPLE_PLAYER_SIZE),
            CollisionLayers::default(),
        ))
        .id();
    let child = stepper
        .server_app
        .world_mut()
        .spawn((
            CompoundChildCollider,
            ChildOf(root),
            Transform::from_translation(EXAMPLE_CHILD_OFFSET.extend(0.0)),
            Collider::rectangle(EXAMPLE_CHILD_SIZE, EXAMPLE_CHILD_SIZE),
            ColliderOf { body: root },
            CollisionLayers::default(),
        ))
        .id();

    stepper.frame_step_server_first(20);

    let predicted_root = mapped_entity(&stepper, 0, root);
    let interpolated_root = mapped_entity(&stepper, 1, root);
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(child)
            .is_none(),
        "the deterministic child entity must not be replicated"
    );
    let predicted_child = local_compound_child(stepper.client_apps[0].world(), predicted_root);
    let interpolated_child =
        local_compound_child(stepper.client_apps[1].world(), interpolated_root);

    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(predicted_root)
            .is_some()
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(predicted_child)
            .is_none()
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Interpolated>(predicted_child)
            .is_none()
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(interpolated_root)
            .is_some()
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(interpolated_child)
            .is_none()
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(interpolated_child)
            .is_none()
    );

    let predicted_world = stepper.client_apps[0].world();
    assert!(predicted_world.get::<RigidBody>(predicted_root).is_some());
    assert!(
        predicted_world
            .get::<ApplyPosToTransform>(predicted_root)
            .is_none(),
        "RigidBody roots are already synchronized by Avian and do not need the opt-in marker"
    );
    assert_eq!(
        predicted_world
            .get::<ColliderOf>(predicted_root)
            .unwrap()
            .body,
        predicted_root,
        "the With<RigidBody> replication filter must retain roots even when Avian gives them ColliderOf"
    );
    assert!(predicted_world.get::<Collider>(predicted_child).is_some());
    assert!(predicted_world.get::<RigidBody>(predicted_child).is_none());
    assert!(predicted_world.get::<Position>(predicted_child).is_some());
    assert!(predicted_world.get::<Rotation>(predicted_child).is_some());
    assert!(
        predicted_world
            .get::<ApplyPosToTransform>(predicted_child)
            .is_none(),
        "a compound child's derived world pose must not overwrite its fixed local Transform"
    );
    assert!(
        predicted_world
            .get::<ConfirmedHistory<Position>>(predicted_child)
            .is_none(),
        "the derived child pose must not acquire an authoritative prediction history"
    );
    assert!(
        predicted_world
            .get::<FrameInterpolationHistory<Position>>(predicted_child)
            .is_none(),
        "only rigid-body roots have a fixed-tick render pose"
    );
    assert_eq!(
        predicted_world
            .get::<ColliderOf>(predicted_child)
            .unwrap()
            .body,
        predicted_root
    );
    assert!(
        predicted_world
            .get::<FrameInterpolationHistory<Position>>(predicted_root)
            .is_some()
    );

    let interpolated_world = stepper.client_apps[1].world();
    assert!(
        interpolated_world
            .get::<ApplyPosToTransform>(interpolated_root)
            .is_some(),
        "a render-only interpolated root still needs Position-to-Transform sync"
    );
    assert!(
        interpolated_world
            .get::<Position>(interpolated_child)
            .is_none(),
        "a non-rigid ColliderOf child's derived Position must not be replicated"
    );
    assert!(
        interpolated_world
            .get::<Rotation>(interpolated_child)
            .is_none(),
        "a non-rigid ColliderOf child's derived Rotation must not be replicated"
    );
    assert!(
        interpolated_world
            .get::<RigidBody>(interpolated_root)
            .is_none()
    );
    assert!(
        interpolated_world
            .get::<Collider>(interpolated_child)
            .is_none()
    );
    assert_touching(
        interpolated_world,
        interpolated_root,
        interpolated_child,
        "interpolated client",
    );
    assert_touching(
        predicted_world,
        predicted_root,
        predicted_child,
        "predicted client",
    );
    assert_touching(stepper.server_app.world(), root, child, "server");

    let rollbacks_before = stepper.client_apps[0]
        .world()
        .resource::<PredictionMetrics>()
        .rollbacks;
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<InjectMisprediction>()
        .0 = true;

    stepper.frame_step_server_first(30);

    let predicted_world = stepper.client_apps[0].world();
    assert!(
        predicted_world.resource::<PredictionMetrics>().rollbacks > rollbacks_before,
        "the forced position error did not trigger a rollback"
    );
    assert!(
        predicted_world.resource::<CorrectionObserved>().0,
        "the rollback did not create a visual correction"
    );

    for (client_id, client) in stepper.client_apps.iter().enumerate() {
        let audit = client.world().resource::<TouchAudit>();
        assert!(audit.samples > 0, "client {client_id} recorded no samples");
        assert!(
            audit.maximum_gap < 0.001,
            "client {client_id} opened a {} unit gap between the compound colliders",
            audit.maximum_gap
        );
        assert!(
            audit.maximum_local_error < 0.001,
            "client {client_id} changed the child collider's fixed local transform by {} units",
            audit.maximum_local_error
        );
    }
    assert_touching(
        predicted_world,
        predicted_root,
        predicted_child,
        "corrected predicted client",
    );
    assert_touching(
        stepper.client_apps[1].world(),
        interpolated_root,
        interpolated_child,
        "interpolated client after correction",
    );
}

/// Both clients must simulate both player rigid-body roots. This specifically
/// guards against representing the remote player as a render-only interpolated
/// entity, which lets locally predicted players pass straight through it.
#[test]
fn every_client_predicts_both_compound_players_and_resolves_their_contact() {
    let mut config = StepperConfig::with_netcode_clients(2);
    config.frame_duration = Duration::from_millis(5);
    let mut stepper = ClientServerStepper::from_config(config);

    *stepper.server_app.world_mut().resource_mut::<Gravity>() = Gravity::ZERO;
    stepper.server_app.init_resource::<PlayerContactObserved>();
    stepper
        .server_app
        .add_systems(FixedLast, record_player_contact);
    for client in &mut stepper.client_apps {
        *client.world_mut().resource_mut::<Gravity>() = Gravity::ZERO;
        client.init_resource::<PlayerContactObserved>();
        client.add_systems(
            PostUpdate,
            materialize_all_predicted_compound_players.after(TransformSystems::Propagate),
        );
        client.add_systems(FixedLast, record_player_contact);
    }

    let left = spawn_colliding_player(
        stepper.server_app.world_mut(),
        10.0,
        Vec2::new(-50.0, 0.0),
        Vec2::new(40.0, 0.0),
    );
    let right = spawn_colliding_player(
        stepper.server_app.world_mut(),
        20.0,
        Vec2::new(30.0, 0.0),
        Vec2::new(-40.0, 0.0),
    );

    stepper.frame_step_server_first(150);

    assert!(
        stepper
            .server_app
            .world()
            .resource::<PlayerContactObserved>()
            .0,
        "the authoritative compound players never collided"
    );
    assert_players_resolved_contact(stepper.server_app.world(), left.0, right.0, "server");

    for (client_id, client) in stepper.client_apps.iter().enumerate() {
        let left_root = mapped_entity(&stepper, client_id, left.0);
        let right_root = mapped_entity(&stepper, client_id, right.0);
        let world = client.world();
        let left_child = local_compound_child(world, left_root);
        let right_child = local_compound_child(world, right_root);

        let mapper = stepper.client(client_id).get::<MessageManager>().unwrap();
        assert!(mapper.entity_mapper.get_local(left.1).is_none());
        assert!(mapper.entity_mapper.get_local(right.1).is_none());

        assert!(
            world.resource::<PlayerContactObserved>().0,
            "client {client_id} never simulated the player-player contact"
        );
        for root in [left_root, right_root] {
            assert!(world.get::<Predicted>(root).is_some());
            assert!(world.get::<RigidBody>(root).is_some());
            assert!(world.get::<ApplyPosToTransform>(root).is_none());
            assert!(
                world
                    .get::<FrameInterpolationHistory<Position>>(root)
                    .is_some()
            );
        }
        for child in [left_child, right_child] {
            assert!(world.get::<Predicted>(child).is_none());
            assert!(world.get::<Interpolated>(child).is_none());
            assert!(world.get::<RigidBody>(child).is_none());
            assert!(world.get::<Collider>(child).is_some());
            assert!(world.get::<Position>(child).is_some());
            assert!(world.get::<Rotation>(child).is_some());
            assert!(world.get::<ApplyPosToTransform>(child).is_none());
        }
        assert_eq!(world.get::<ColliderOf>(left_child).unwrap().body, left_root);
        assert_eq!(
            world.get::<ColliderOf>(right_child).unwrap().body,
            right_root
        );
        assert_touching(
            world,
            left_root,
            left_child,
            &format!("client {client_id} left player"),
        );
        assert_touching(
            world,
            right_root,
            right_child,
            &format!("client {client_id} right player"),
        );
        assert_players_resolved_contact(
            world,
            left_root,
            right_root,
            &format!("client {client_id}"),
        );
    }
}

fn spawn_colliding_player(
    world: &mut World,
    id: f32,
    position: Vec2,
    velocity: Vec2,
) -> (Entity, Entity) {
    let root = world
        .spawn((
            CompA(id),
            Replicate::to_clients(NetworkTarget::All),
            DisableReplicateHierarchy,
            PredictionTarget::to_clients(NetworkTarget::All),
            RigidBody::Dynamic,
            Position::from(position),
            Rotation::default(),
            Transform::from_translation(position.extend(0.0)),
            LinearVelocity(velocity),
            AngularVelocity::ZERO,
            Collider::rectangle(EXAMPLE_PLAYER_SIZE, EXAMPLE_PLAYER_SIZE),
            Restitution::new(0.3),
            CollisionLayers::default(),
        ))
        .id();
    let child = world
        .spawn((
            CompoundChildCollider,
            ChildOf(root),
            Transform::from_translation(EXAMPLE_CHILD_OFFSET.extend(0.0)),
            Collider::rectangle(EXAMPLE_CHILD_SIZE, EXAMPLE_CHILD_SIZE),
            ColliderOf { body: root },
            Restitution::new(0.3),
            CollisionLayers::default(),
        ))
        .id();
    (root, child)
}

#[derive(Component)]
struct AllPredictedPlayerReady;

fn materialize_all_predicted_compound_players(
    mut commands: Commands,
    roots: Query<(Entity, &CompA), (With<Predicted>, Without<AllPredictedPlayerReady>)>,
    physics_children: Query<
        (Entity, &ChildOf),
        (
            With<CompoundChildCollider>,
            With<ExampleChildTransformReady>,
            Without<ExampleChildPhysicsReady>,
        ),
    >,
    predicted_roots: Query<(), (With<Predicted>, With<RigidBody>)>,
) {
    for (entity, player) in &roots {
        let velocity = if player.0 < 15.0 {
            Vec2::new(40.0, 0.0)
        } else {
            Vec2::new(-40.0, 0.0)
        };
        commands.entity(entity).insert((
            AllPredictedPlayerReady,
            RigidBody::Dynamic,
            LinearVelocity(velocity),
            AngularVelocity::ZERO,
            Collider::rectangle(EXAMPLE_PLAYER_SIZE, EXAMPLE_PLAYER_SIZE),
            Restitution::new(0.3),
            CollisionLayers::default(),
            FrameInterpolate,
        ));
        commands.spawn((
            CompoundChildCollider,
            ChildOf(entity),
            Transform::from_translation(EXAMPLE_CHILD_OFFSET.extend(0.0)),
            ExampleChildTransformReady,
        ));
    }
    for (entity, child_of) in &physics_children {
        if !predicted_roots.contains(child_of.parent()) {
            continue;
        }
        commands.entity(entity).insert((
            Collider::rectangle(EXAMPLE_CHILD_SIZE, EXAMPLE_CHILD_SIZE),
            ColliderOf {
                body: child_of.parent(),
            },
            Restitution::new(0.3),
            CollisionLayers::default(),
            ExampleChildPhysicsReady,
        ));
    }
}

#[derive(Resource, Default)]
struct PlayerContactObserved(bool);

fn record_player_contact(
    graph: Res<ContactGraph>,
    roots: Query<&CompA>,
    colliders: Query<Entity, With<Collider>>,
    mut observed: ResMut<PlayerContactObserved>,
) {
    if observed.0 {
        return;
    }
    observed.0 = colliders.iter().any(|collider| {
        graph.contact_pairs_with(collider).any(|pair| {
            if !pair.is_touching() {
                return false;
            }
            let (Some(body1), Some(body2)) = (pair.body1, pair.body2) else {
                return false;
            };
            let (Ok(player1), Ok(player2)) = (roots.get(body1), roots.get(body2)) else {
                return false;
            };
            (player1.0 - player2.0).abs() > f32::EPSILON
        })
    });
}

fn assert_players_resolved_contact(world: &World, left: Entity, right: Entity, label: &str) {
    let left_position = world.get::<Position>(left).unwrap();
    let right_position = world.get::<Position>(right).unwrap();
    let left_velocity = world.get::<LinearVelocity>(left).unwrap();
    let right_velocity = world.get::<LinearVelocity>(right).unwrap();
    assert!(
        left_position.x < right_position.x,
        "{label}: the players passed through each other: left={left_position:?}, right={right_position:?}"
    );
    assert!(
        left_velocity.x < 0.0 && right_velocity.x > 0.0,
        "{label}: the collision did not reverse their approach: left={left_velocity:?}, right={right_velocity:?}"
    );
}

#[derive(Component)]
struct ExampleRootReady;

#[derive(Component)]
struct ExampleChildTransformReady;

#[derive(Component)]
struct ExampleChildPhysicsReady;

#[derive(Resource, Default)]
struct TouchAudit {
    samples: usize,
    maximum_gap: f32,
    maximum_local_error: f32,
}

#[derive(Resource, Default)]
struct CorrectionObserved(bool);

#[derive(Resource, Default)]
struct InjectMisprediction(bool);

fn inject_fixed_misprediction(
    mut inject: ResMut<InjectMisprediction>,
    mut roots: Query<(&mut Position, &mut Transform), (With<CompA>, With<Predicted>)>,
) {
    if !core::mem::take(&mut inject.0) {
        return;
    }
    for (mut position, mut transform) in &mut roots {
        position.x += 100.0;
        transform.translation.x += 100.0;
    }
}

fn materialize_example_compound_player(
    mut commands: Commands,
    roots: Query<(Entity, Has<Predicted>), (With<CompA>, Without<ExampleRootReady>)>,
    predicted_children: Query<
        (Entity, &ChildOf),
        (
            With<CompoundChildCollider>,
            With<ExampleChildTransformReady>,
            Without<ExampleChildPhysicsReady>,
        ),
    >,
    predicted_roots: Query<(), (With<Predicted>, With<RigidBody>)>,
) {
    for (entity, predicted) in &roots {
        let mut entity_commands = commands.entity(entity);
        entity_commands.insert(ExampleRootReady);
        if predicted {
            entity_commands.insert((
                RigidBody::Kinematic,
                LinearVelocity(Vec2::new(20.0, 4.0)),
                AngularVelocity(0.4),
                Collider::rectangle(EXAMPLE_PLAYER_SIZE, EXAMPLE_PLAYER_SIZE),
                CollisionLayers::default(),
                FrameInterpolate,
            ));
        }
        commands.spawn((
            CompoundChildCollider,
            ChildOf(entity),
            Transform::from_translation(EXAMPLE_CHILD_OFFSET.extend(0.0)),
            ExampleChildTransformReady,
        ));
    }

    // ColliderOf computes ColliderTransform from the propagated world transforms.
    // Wait one render frame after installing the local Transform before attaching
    // predicted physics, matching the example's local template materialization path.
    for (entity, child_of) in &predicted_children {
        if !predicted_roots.contains(child_of.parent()) {
            continue;
        }
        commands.entity(entity).insert((
            Collider::rectangle(EXAMPLE_CHILD_SIZE, EXAMPLE_CHILD_SIZE),
            ColliderOf {
                body: child_of.parent(),
            },
            CollisionLayers::default(),
            ExampleChildPhysicsReady,
        ));
    }
}

fn record_touching_faces(
    roots: Query<&GlobalTransform, (With<CompA>, With<ExampleRootReady>)>,
    children: Query<
        (&ChildOf, &Transform, &GlobalTransform),
        (
            With<CompoundChildCollider>,
            With<ExampleChildTransformReady>,
        ),
    >,
    mut audit: ResMut<TouchAudit>,
) {
    for (child_of, child_local, child_global) in &children {
        let Ok(root_global) = roots.get(child_of.parent()) else {
            continue;
        };
        let gap = touching_face_gap(root_global, child_global);
        let local_error = child_local
            .translation
            .distance(EXAMPLE_CHILD_OFFSET.extend(0.0))
            .max(child_local.rotation.angle_between(Quat::IDENTITY));
        audit.samples += 1;
        audit.maximum_gap = audit.maximum_gap.max(gap);
        audit.maximum_local_error = audit.maximum_local_error.max(local_error);
    }
}

fn record_visual_correction(
    corrections: Query<(), With<VisualCorrection<Position>>>,
    mut observed: ResMut<CorrectionObserved>,
) {
    observed.0 |= !corrections.is_empty();
}

fn mapped_entity(stepper: &ClientServerStepper, client_id: usize, server: Entity) -> Entity {
    stepper
        .client(client_id)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server)
        .unwrap()
}

fn local_compound_child(world: &World, root: Entity) -> Entity {
    let mut children = world.iter_entities().filter_map(|entity| {
        let child_of = entity.get::<ChildOf>()?;
        (entity.contains::<CompoundChildCollider>() && child_of.parent() == root)
            .then_some(entity.id())
    });
    let child = children
        .next()
        .unwrap_or_else(|| panic!("no deterministic compound child exists for root {root:?}"));
    assert!(
        children.next().is_none(),
        "more than one deterministic compound child exists for root {root:?}"
    );
    child
}

fn assert_touching(world: &World, root: Entity, child: Entity, label: &str) {
    let root_global = world.get::<GlobalTransform>(root).unwrap();
    let child_global = world.get::<GlobalTransform>(child).unwrap();
    let child_local = world.get::<Transform>(child).unwrap();
    assert_relative_eq!(
        child_local.translation,
        EXAMPLE_CHILD_OFFSET.extend(0.0),
        epsilon = 0.001
    );
    assert_relative_eq!(child_local.rotation, Quat::IDENTITY, epsilon = 0.001);
    let gap = touching_face_gap(root_global, child_global);
    assert!(
        gap < 0.001,
        "{label}: the compound collider faces are {gap} units apart; root local/global: {:?}/{root_global:?}; child local/global: {:?}/{child_global:?}",
        world.get::<Transform>(root),
        world.get::<Transform>(child),
    );
}

fn touching_face_gap(root: &GlobalTransform, child: &GlobalTransform) -> f32 {
    let root = root.compute_transform();
    let child = child.compute_transform();
    let root_face = root.transform_point(Vec3::new(EXAMPLE_PLAYER_SIZE / 2.0, 0.0, 0.0));
    let child_face = child.transform_point(Vec3::new(-EXAMPLE_CHILD_SIZE / 2.0, 0.0, 0.0));
    root_face.distance(child_face)
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

    // This test reconstructs local-only physics on the remote hierarchy. Insert
    // ColliderOf explicitly so the nested collider is
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
