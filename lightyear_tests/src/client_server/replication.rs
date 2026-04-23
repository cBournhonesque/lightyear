//! Check various replication scenarios between 2 peers only

use crate::protocol::{CompA, CompCustomInterp, CompReplicateOnce};
use crate::stepper::*;
use bevy::prelude::{Bundle, Entity, Name, World};
use bevy_replicon::prelude::Replicated;
use lightyear::prelude::ConfirmedHistory;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prediction::Predicted;
use lightyear_core::prelude::LocalTimeline;
use lightyear_messages::MessageManager;
use lightyear_replication::control::{ControlledBy, ControlledByRemote};
use lightyear_replication::prelude::*;
use lightyear_sync::prelude::InputTimeline;
use test_log::test;
use tracing::info;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplicationDirection {
    ServerToClient,
    ClientToServer,
}

impl ReplicationDirection {
    fn replicate(self) -> Replicate {
        match self {
            Self::ServerToClient => Replicate::to_clients(NetworkTarget::All),
            Self::ClientToServer => Replicate::to_server(),
        }
    }

    fn propagation_frames(self) -> usize {
        match self {
            Self::ServerToClient => 2,
            Self::ClientToServer => 1,
        }
    }
}

fn active_replication_directions() -> impl Iterator<Item = ReplicationDirection> {
    // Released Replicon does not support Lightyear's current same-world
    // client->server replication path. Keep the generic harness in place so the
    // symmetric coverage is ready once that direction is re-enabled.
    [ReplicationDirection::ServerToClient].into_iter()
}

fn with_source_world<R>(
    stepper: &mut ClientServerStepper,
    direction: ReplicationDirection,
    f: impl FnOnce(&mut World) -> R,
) -> R {
    match direction {
        ReplicationDirection::ServerToClient => f(stepper.server_app.world_mut()),
        ReplicationDirection::ClientToServer => f(stepper.client_app().world_mut()),
    }
}

fn with_target_world<R>(
    stepper: &mut ClientServerStepper,
    direction: ReplicationDirection,
    f: impl FnOnce(&mut World) -> R,
) -> R {
    match direction {
        ReplicationDirection::ServerToClient => f(stepper.client_apps[0].world_mut()),
        ReplicationDirection::ClientToServer => f(stepper.server_app.world_mut()),
    }
}

fn target_entity(
    stepper: &ClientServerStepper,
    direction: ReplicationDirection,
    source_entity: Entity,
) -> Option<Entity> {
    match direction {
        ReplicationDirection::ServerToClient => stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(source_entity),
        ReplicationDirection::ClientToServer => stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(source_entity),
    }
}

fn spawn_on_source<B: Bundle>(
    stepper: &mut ClientServerStepper,
    direction: ReplicationDirection,
    bundle: B,
) -> Entity {
    with_source_world(stepper, direction, |world| world.spawn(bundle).id())
}

#[test]
fn test_spawn() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity = spawn_on_source(&mut stepper, direction, (direction.replicate(),));
        stepper.frame_step(direction.propagation_frames());

        target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));
    }
}

#[test]
fn test_spawn_from_replicate_change() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity = spawn_on_source(&mut stepper, direction, (Replicate::manual(vec![]),));
        stepper.frame_step(direction.propagation_frames());
        assert!(
            target_entity(&stepper, direction, source_entity).is_none(),
            "entity should not be replicated yet for {direction:?}"
        );

        with_source_world(&mut stepper, direction, |world| {
            world
                .entity_mut(source_entity)
                .insert(direction.replicate());
        });
        stepper.frame_step(direction.propagation_frames());

        target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));
    }
}

/// When client 2 connects:
/// - the existing entities are replicated to the new client
#[test]
fn test_spawn_new_connection() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    stepper.frame_step(2);
    stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();

    // second client connects
    stepper.new_client(ClientType::Netcode, None);
    stepper.init();

    // make sure the entity is also replicated to the newly connected client
    stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");
}

#[test]
fn test_entity_despawn() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity = spawn_on_source(&mut stepper, direction, (direction.replicate(),));
        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));

        with_source_world(&mut stepper, direction, |world| {
            world.despawn(source_entity);
        });
        stepper.frame_step(direction.propagation_frames());

        let mirrored_exists = with_target_world(&mut stepper, direction, |world| {
            world.get_entity(mirrored_entity).is_ok()
        });
        assert!(
            !mirrored_exists,
            "entity should be despawned on the target for {direction:?}"
        );
    }
}

#[test]
fn test_despawn_from_replicate_change() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity = spawn_on_source(&mut stepper, direction, (direction.replicate(),));
        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));

        with_source_world(&mut stepper, direction, |world| {
            world
                .entity_mut(source_entity)
                .insert(Replicate::manual(vec![]));
        });
        stepper.frame_step(direction.propagation_frames());

        let mirrored_exists = with_target_world(&mut stepper, direction, |world| {
            world.get_entity(mirrored_entity).is_ok()
        });
        assert!(
            !mirrored_exists,
            "entity should be despawned after replication target removal for {direction:?}"
        );
    }
}

#[test]
fn test_component_insert() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity = spawn_on_source(&mut stepper, direction, (direction.replicate(),));
        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));

        with_source_world(&mut stepper, direction, |world| {
            world.entity_mut(source_entity).insert(CompA(1.0));
        });
        stepper.frame_step(direction.propagation_frames());

        let mirrored_comp = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().cloned()
        });
        assert_eq!(
            mirrored_comp,
            Some(CompA(1.0)),
            "component insert should replicate for {direction:?}"
        );
    }
}

#[test]
fn test_component_update() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity =
            spawn_on_source(&mut stepper, direction, (direction.replicate(), CompA(1.0)));
        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));
        let initial_comp = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().cloned()
        });
        assert_eq!(
            initial_comp,
            Some(CompA(1.0)),
            "initial component state should replicate for {direction:?}"
        );

        with_source_world(&mut stepper, direction, |world| {
            world
                .entity_mut(source_entity)
                .get_mut::<CompA>()
                .unwrap()
                .0 = 2.0;
        });
        stepper.frame_step(direction.propagation_frames());

        let updated_comp = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().cloned()
        });
        assert_eq!(
            updated_comp,
            Some(CompA(2.0)),
            "component update should replicate for {direction:?}"
        );
    }
}

#[test]
#[ignore = "requires client->server replication support on released replicon"]
fn test_client_owned_entity_rebroadcasts_updates_to_other_clients() {
    use lightyear_core::id::RemoteId;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_0_id = stepper.client_of(0).get::<RemoteId>().unwrap().0;
    let client_entity = stepper.client_apps[0]
        .world_mut()
        .spawn((Replicate::to_server(), CompA(1.0)))
        .id();

    stepper.frame_step(1);

    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(Replicate::to_clients(NetworkTarget::AllExceptSingle(
            client_0_id,
        )));

    stepper.frame_step(2);

    let client_1_entity = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("client 1 should receive the rebroadcast entity");
    assert_eq!(
        stepper.client_apps[1].world().get::<CompA>(client_1_entity),
        Some(&CompA(1.0))
    );

    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity)
        .insert(CompA(2.0));
    stepper.frame_step(2);

    assert_eq!(
        stepper.server_app.world().get::<CompA>(server_entity),
        Some(&CompA(2.0)),
        "server should keep receiving updates from the owning client"
    );
    assert_eq!(
        stepper.client_apps[1].world().get::<CompA>(client_1_entity),
        Some(&CompA(2.0)),
        "rebroadcast client should receive subsequent updates"
    );
}

#[test]
fn test_custom_interpolation_component_gets_confirmed_history() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
            CompCustomInterp(1.0),
        ))
        .id();

    stepper.frame_step(2);
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompCustomInterp(2.0));
    stepper.frame_step_server_first(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();
    let client_entity_ref = stepper.client_apps[0].world().entity(client_entity);

    assert!(
        client_entity_ref.get::<Interpolated>().is_some(),
        "entity should be interpolated on the client"
    );
    let history = client_entity_ref
        .get::<ConfirmedHistory<CompCustomInterp>>()
        .expect(
            "custom-interpolated components should get ConfirmedHistory on interpolated entities",
        );
    assert!(
        history.start().is_some(),
        "custom-interpolated history should contain at least one confirmed update"
    );
}

#[test]
fn test_late_join_client_gets_predicted_marker_for_prediction_target_all() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            CompA(1.0),
        ))
        .id();

    stepper.frame_step(3);

    let client_0_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(client_0_entity)
            .is_some(),
        "existing client should see the entity as predicted"
    );

    stepper.new_client(ClientType::Netcode, None);
    stepper.init();
    stepper.frame_step(3);

    let client_1_entity = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("late-joining client should receive the entity");
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(client_1_entity)
            .is_some(),
        "late-joining client should see PredictionTarget::All entity as predicted"
    );
}

#[test]
fn test_late_join_client_gets_latest_state_for_existing_predicted_entity() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            CompA(0.0),
        ))
        .id();

    stepper.frame_step(2);

    for value in [1.0, 2.0, 3.0, 4.0] {
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(CompA(value));
        stepper.frame_step(1);
    }

    stepper.new_client(ClientType::Netcode, None);
    stepper.init();
    stepper.frame_step(3);

    let client_1_entity = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("late-joining client should receive the entity");
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(client_1_entity)
            .is_some(),
        "late-joining client should see the entity as predicted"
    );
    assert_eq!(
        stepper.client_apps[1].world().get::<CompA>(client_1_entity),
        Some(&CompA(4.0)),
        "late-joining client should receive the latest component state"
    );
}

/// Test that replicating updates works even after a large tick jump.
///
/// With u32 ticks, wrapping takes ~828 days at 60 Hz so it is not a practical
/// concern. This test verifies that a moderate jump (10 000 ticks) does not
/// break replication.
#[test]
fn test_component_update_after_tick_wrap() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
        // remove InputTimeline otherwise it will try to resync
        stepper.client_mut(0).remove::<InputTimeline>();

        let source_entity =
            spawn_on_source(&mut stepper, direction, (direction.replicate(), CompA(1.0)));

        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));

        // Apply a large tick jump on both client and server
        stepper
            .client_app()
            .world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10_000);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10_000);
        stepper.frame_step(direction.propagation_frames());

        stepper
            .client_app()
            .world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10_000);
        stepper
            .server_app
            .world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10_000);
        stepper.frame_step(direction.propagation_frames());

        with_source_world(&mut stepper, direction, |world| {
            world
                .entity_mut(source_entity)
                .get_mut::<CompA>()
                .unwrap()
                .0 = 2.0;
        });
        stepper.frame_step(direction.propagation_frames());

        let updated_comp = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().cloned()
        });
        assert_eq!(
            updated_comp,
            Some(CompA(2.0)),
            "component update should survive tick jump for {direction:?}"
        );
    }
}

#[test]
fn test_component_remove() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity =
            spawn_on_source(&mut stepper, direction, (direction.replicate(), CompA(1.0)));
        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));
        let initial_comp = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().cloned()
        });
        assert_eq!(
            initial_comp,
            Some(CompA(1.0)),
            "initial component state should replicate for {direction:?}"
        );

        with_source_world(&mut stepper, direction, |world| {
            world.entity_mut(source_entity).remove::<CompA>();
        });
        stepper.frame_step(direction.propagation_frames());

        let removed = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().is_none()
        });
        assert!(
            removed,
            "component remove should replicate for {direction:?}"
        );
    }
}

/// Check that component removes are not replicated if the entity does not have Replicating
/// TODO: removing Replicated with replicon causes a despawn on remote, not a pause
#[test]
#[ignore]
fn test_component_remove_not_replicating() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompA(1.0)))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(1.0)
    );

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        // TODO: removing Replicated will pause the replication instead of sending a despawn
        .remove::<Replicated>();
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .remove::<CompA>();
    stepper.frame_step(1);
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .is_some()
    );
}

/// Check that if we remove a non-replicated component, the replicate component does not get removed
#[test]
fn test_component_remove_non_replicated() {
    for direction in active_replication_directions() {
        let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

        let source_entity = spawn_on_source(
            &mut stepper,
            direction,
            (direction.replicate(), CompA(1.0), Name::from("a")),
        );
        stepper.frame_step(direction.propagation_frames());
        let mirrored_entity = target_entity(&stepper, direction, source_entity)
            .unwrap_or_else(|| panic!("entity is not present in entity map for {direction:?}"));
        let initial_comp = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().cloned()
        });
        assert_eq!(
            initial_comp,
            Some(CompA(1.0)),
            "initial component state should replicate for {direction:?}"
        );

        with_source_world(&mut stepper, direction, |world| {
            world.entity_mut(source_entity).remove::<Name>();
        });
        stepper.frame_step(direction.propagation_frames());

        let comp_still_present = with_target_world(&mut stepper, direction, |world| {
            world.entity(mirrored_entity).get::<CompA>().is_some()
        });
        assert!(
            comp_still_present,
            "removing a non-replicated component should not affect replicated state for {direction:?}"
        );
    }
}

// /// Test that a component removal is not replicated if the component is marked as disabled
// #[test]
// fn test_component_remove_disabled() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     let client_entity = stepper
//         .client_app()
//         .world_mut()
//         .spawn((Replicate::to_server(), CompA(1.0)))
//         .id();
//     stepper.frame_step(1);
//     let server_entity = stepper
//         .client_of(0)
//         .get::<MessageManager>()
//         .unwrap()
//         .entity_mapper
//         .get_local(client_entity)
//         .unwrap();
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompA>()
//             .expect("component missing"),
//         &CompA(1.0)
//     );
//
//     let mut overrides = ComponentReplicationOverrides::<CompA>::default();
//     overrides.global_override(ComponentReplicationOverride {
//         disable: true,
//         ..default()
//     });
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .insert(overrides);
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .remove::<CompA>();
//     stepper.frame_step(1);
//     // the removal was not replicated since the component replication was disabled
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompA>()
//             .expect("component missing"),
//         &CompA(1.0)
//     );
// }

// #[test]
// fn test_component_disabled() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     let client_entity = stepper
//         .client_app()
//         .world_mut()
//         .spawn((Replicate::to_server(), CompDisabled(1.0)))
//         .id();
//     stepper.frame_step(1);
//
//     let server_entity = stepper
//         .client_of(0)
//         .get::<MessageManager>()
//         .unwrap()
//         .entity_mapper
//         .get_local(client_entity)
//         .unwrap();
//     assert!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompDisabled>()
//             .is_none()
//     );
// }

/// TODO: CompReplicateOnce not registered in replicon yet
#[test]
#[ignore]
fn test_component_replicate_once() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompReplicateOnce(1.0)))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompReplicateOnce>()
            .expect("component missing"),
        &CompReplicateOnce(1.0)
    );

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<CompReplicateOnce>()
        .unwrap()
        .0 = 2.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompReplicateOnce>()
            .expect("component missing"),
        &CompReplicateOnce(1.0)
    );
}

// /// Default = replicate_once
// /// GlobalOverride = replicate_always
// /// PerSenderOverride = replicate_once
// #[test]
// fn test_component_replicate_once_overrides() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     let client_entity = stepper
//         .client_app()
//         .world_mut()
//         .spawn((Replicate::to_server(), CompReplicateOnce(1.0)))
//         .id();
//     stepper.frame_step(1);
//     let server_entity = stepper
//         .client_of(0)
//         .get::<MessageManager>()
//         .unwrap()
//         .entity_mapper
//         .get_local(client_entity)
//         .unwrap();
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompReplicateOnce>()
//             .expect("component missing"),
//         &CompReplicateOnce(1.0)
//     );
//
//     let mut overrides = ComponentReplicationOverrides::<CompReplicateOnce>::default();
//     overrides.global_override(ComponentReplicationOverride {
//         replicate_always: true,
//         ..default()
//     });
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .insert(overrides);
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .get_mut::<CompReplicateOnce>()
//         .unwrap()
//         .0 = 2.0;
//     stepper.frame_step(1);
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompReplicateOnce>()
//             .expect("component missing"),
//         &CompReplicateOnce(2.0)
//     );
//
//     stepper.client_apps[0]
//         .world_mut()
//         .entity_mut(client_entity)
//         .get_mut::<ComponentReplicationOverrides<CompReplicateOnce>>()
//         .unwrap()
//         .override_for_sender(
//             ComponentReplicationOverride {
//                 replicate_once: true,
//                 ..default()
//             },
//             stepper.client_entities[0],
//         );
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .get_mut::<CompReplicateOnce>()
//         .unwrap()
//         .0 = 3.0;
//     stepper.frame_step(1);
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompReplicateOnce>()
//             .expect("component missing"),
//         &CompReplicateOnce(2.0)
//     );
// }

// /// Default = disabled
// /// GlobalOverride = enabled
// /// PerSenderOverride = disabled
// #[test]
// fn test_component_disabled_overrides() {
//     let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
//
//     let client_entity = stepper
//         .client_app()
//         .world_mut()
//         .spawn((Replicate::to_server(), CompDisabled(1.0)))
//         .id();
//     stepper.frame_step(1);
//     let server_entity = stepper
//         .client_of(0)
//         .get::<MessageManager>()
//         .unwrap()
//         .entity_mapper
//         .get_local(client_entity)
//         .unwrap();
//     assert!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompDisabled>()
//             .is_none()
//     );
//
//     info!("enabled global");
//     let mut overrides = ComponentReplicationOverrides::<CompDisabled>::default();
//     overrides.global_override(ComponentReplicationOverride {
//         enable: true,
//         ..default()
//     });
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .insert(overrides);
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .get_mut::<CompDisabled>()
//         .unwrap()
//         .0 = 2.0;
//     stepper.frame_step(1);
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompDisabled>()
//             .expect("component missing"),
//         &CompDisabled(2.0)
//     );
//
//     info!("disabled for sender");
//     stepper.client_apps[0]
//         .world_mut()
//         .entity_mut(client_entity)
//         .get_mut::<ComponentReplicationOverrides<CompDisabled>>()
//         .unwrap()
//         .override_for_sender(
//             ComponentReplicationOverride {
//                 disable: true,
//                 ..default()
//             },
//             stepper.client_entities[0],
//         );
//     stepper
//         .client_app()
//         .world_mut()
//         .entity_mut(client_entity)
//         .get_mut::<CompDisabled>()
//         .unwrap()
//         .0 = 3.0;
//     stepper.frame_step(1);
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .entity(server_entity)
//             .get::<CompDisabled>()
//             .expect("component missing"),
//         &CompDisabled(2.0)
//     );
// }

/// When a client disconnects, entities it controlled (via ControlledBy with SessionBased lifetime)
/// should be despawned on the server.
#[test]
fn test_controlled_entity_despawned_on_server_when_client_disconnects() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_of_1 = stepper.client_of(1).id();
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_1,
                lifetime: Default::default(),
            },
        ))
        .id();
    assert!(stepper.client_of(1).get::<ControlledByRemote>().is_some());

    // the server entity is replicated to both clients
    stepper.frame_step(2);
    stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    // client 1 disconnects
    stepper.disconnect_client();
    stepper.frame_step(2);

    // the entity controlled by client 1 should be despawned on the server
    assert!(
        stepper
            .server_app
            .world()
            .get_entity(server_entity)
            .is_err(),
        "Entity controlled by disconnected client should be despawned on the server"
    );
}

/// When a client disconnects, all replicated entities on that client should be despawned.
#[test]
fn test_replicated_entities_despawned_on_client_when_client_disconnects() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    // server spawns an entity replicated to all clients
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    stepper.frame_step(2);

    // verify the entity exists on client 0
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity should be replicated to client 0");
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Replicated>(client_entity)
            .is_some(),
        "Entity should have Replicated component"
    );

    // disconnect client 1 (the last one), this pops client 1
    stepper.disconnect_client();
    stepper.frame_step(2);

    // client 0 should still have the entity (it didn't disconnect)
    assert!(
        stepper.client_apps[0]
            .world()
            .get_entity(client_entity)
            .is_ok(),
        "Entity should still exist on client 0 since it didn't disconnect"
    );
}

/// When a client disconnects, entities that were replicated from the server
/// should be despawned on that client (via replicon's ClientState transition).
///
/// We can't use `disconnect_client()` here because it pops the client app.
/// Instead, we manually trigger the disconnect on the client side and keep running it.
#[test]
fn test_all_replicated_despawned_on_disconnecting_client() {
    use lightyear_connection::client::Disconnect;
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    // server spawns an entity replicated to all clients
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    stepper.frame_step(2);

    // verify the entity exists on client 1
    let client_entity_on_1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity should be replicated to client 1");
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Replicated>(client_entity_on_1)
            .is_some(),
        "Entity should have Replicated component on client 1"
    );

    // Manually trigger disconnect on client 1 without removing it from the stepper
    let client_1_entity = stepper.client_entities[1];
    stepper.client_apps[1].world_mut().trigger(Disconnect {
        entity: client_1_entity,
    });
    // Also insert Disconnected on the server side for client_of 1
    let client_of_1 = stepper.client_of_entities[1];
    stepper
        .server_app
        .world_mut()
        .entity_mut(client_of_1)
        .insert(lightyear_connection::client::Disconnected { reason: None });
    stepper.server_app.world_mut().flush();

    // Run a few frames to let the state transition happen
    // We need to update client 1 manually since frame_step updates all clients
    stepper.frame_step(3);

    // After disconnect, all replicated entities should be despawned on client 1
    assert!(
        stepper.client_apps[1]
            .world()
            .get_entity(client_entity_on_1)
            .is_err(),
        "Replicated entity should be despawned on client 1 after disconnect"
    );
}

/// With 2 clients, PredictionTarget::to_clients(NetworkTarget::Single(client_id)) should
/// only give the Predicted component to that specific client, not all clients.
#[test]
fn test_prediction_target_visibility_with_two_clients() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_0_id = stepper
        .client_of(0)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;
    let client_1_id = stepper
        .client_of(1)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;

    // server spawns entity for client 0: predicted for client 0, interpolated for client 1
    let entity_for_client_0 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_0_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_0_id)),
        ))
        .id();

    // server spawns entity for client 1: predicted for client 1, interpolated for client 0
    let entity_for_client_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_1_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_1_id)),
        ))
        .id();

    stepper.frame_step(4);

    // On client 0:
    let mm_0 = stepper.client(0).get::<MessageManager>().unwrap();
    let e0_on_client_0 = mm_0
        .entity_mapper
        .get_local(entity_for_client_0)
        .expect("entity_for_client_0 should be replicated to client 0");
    let e1_on_client_0 = mm_0
        .entity_mapper
        .get_local(entity_for_client_1)
        .expect("entity_for_client_1 should be replicated to client 0");

    // entity_for_client_0 should have Predicted on client 0
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e0_on_client_0)
            .is_some(),
        "entity_for_client_0 should have Predicted on client 0"
    );
    // entity_for_client_1 should NOT have Predicted on client 0
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e1_on_client_0)
            .is_none(),
        "entity_for_client_1 should NOT have Predicted on client 0 (should be Interpolated)"
    );
    // entity_for_client_1 should have Interpolated on client 0
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Interpolated>(e1_on_client_0)
            .is_some(),
        "entity_for_client_1 should have Interpolated on client 0"
    );
    // entity_for_client_0 should NOT have Interpolated on client 0
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Interpolated>(e0_on_client_0)
            .is_none(),
        "entity_for_client_0 should NOT have Interpolated on client 0"
    );

    // On client 1:
    let mm_1 = stepper.client(1).get::<MessageManager>().unwrap();
    let e0_on_client_1 = mm_1
        .entity_mapper
        .get_local(entity_for_client_0)
        .expect("entity_for_client_0 should be replicated to client 1");
    let e1_on_client_1 = mm_1
        .entity_mapper
        .get_local(entity_for_client_1)
        .expect("entity_for_client_1 should be replicated to client 1");

    // entity_for_client_1 should have Predicted on client 1
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(e1_on_client_1)
            .is_some(),
        "entity_for_client_1 should have Predicted on client 1"
    );
    // entity_for_client_0 should NOT have Predicted on client 1
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(e0_on_client_1)
            .is_none(),
        "entity_for_client_0 should NOT have Predicted on client 1 (should be Interpolated)"
    );
    // entity_for_client_0 should have Interpolated on client 1
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(e0_on_client_1)
            .is_some(),
        "entity_for_client_0 should have Interpolated on client 1"
    );
    // entity_for_client_1 should NOT have Interpolated on client 1
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(e1_on_client_1)
            .is_none(),
        "entity_for_client_1 should NOT have Interpolated on client 1"
    );

    // Count predicted entities per client - each should have exactly 1
    let predicted_count_client_0 = stepper.client_apps[0]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[0].world())
        .count();
    assert_eq!(
        predicted_count_client_0, 1,
        "Client 0 should have exactly 1 predicted entity, got {predicted_count_client_0}"
    );

    let predicted_count_client_1 = stepper.client_apps[1]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[1].world())
        .count();
    assert_eq!(
        predicted_count_client_1, 1,
        "Client 1 should have exactly 1 predicted entity, got {predicted_count_client_1}"
    );
}

/// Same as test_prediction_target_visibility_with_two_clients but entities are
/// spawned sequentially (first entity replicated before second spawns), which
/// more closely mimics how the simple_box example works.
#[test]
fn test_prediction_target_visibility_sequential_spawn() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_0_id = stepper
        .client_of(0)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;
    let client_1_id = stepper
        .client_of(1)
        .get::<lightyear_core::id::RemoteId>()
        .unwrap()
        .0;

    // server spawns entity for client 0 first
    let entity_for_client_0 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_0_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_0_id)),
        ))
        .id();

    // Let entity 0 be fully replicated before spawning entity 1
    stepper.frame_step(3);

    // Now spawn entity for client 1
    let entity_for_client_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_1_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_1_id)),
        ))
        .id();

    stepper.frame_step(3);

    // On client 0: should have exactly 1 predicted entity
    let predicted_count_client_0 = stepper.client_apps[0]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[0].world())
        .count();
    assert_eq!(
        predicted_count_client_0, 1,
        "Client 0 should have exactly 1 predicted entity, got {predicted_count_client_0}"
    );

    // On client 1: should have exactly 1 predicted entity
    let predicted_count_client_1 = stepper.client_apps[1]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[1].world())
        .count();
    assert_eq!(
        predicted_count_client_1, 1,
        "Client 1 should have exactly 1 predicted entity, got {predicted_count_client_1}"
    );

    // Verify correct assignment
    let mm_0 = stepper.client(0).get::<MessageManager>().unwrap();
    let e0_on_client_0 = mm_0.entity_mapper.get_local(entity_for_client_0).unwrap();
    let e1_on_client_0 = mm_0.entity_mapper.get_local(entity_for_client_1).unwrap();
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e0_on_client_0)
            .is_some()
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e1_on_client_0)
            .is_none()
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Interpolated>(e1_on_client_0)
            .is_some()
    );

    let mm_1 = stepper.client(1).get::<MessageManager>().unwrap();
    let e0_on_client_1 = mm_1.entity_mapper.get_local(entity_for_client_0).unwrap();
    let e1_on_client_1 = mm_1.entity_mapper.get_local(entity_for_client_1).unwrap();
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(e1_on_client_1)
            .is_some()
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(e0_on_client_1)
            .is_none()
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(e0_on_client_1)
            .is_some()
    );
}

/// Mimics the simple_box pattern: entities spawned via observer on Connected,
/// with ControlledBy + PredictionTarget/InterpolationTarget.
/// Verifies each client sees exactly 1 Predicted entity (their own) and
/// the other client's entity as Interpolated.
#[test]
fn test_simple_box_pattern_prediction_visibility() {
    use lightyear_connection::client::Connected;
    use lightyear_connection::client_of::ClientOf;
    use lightyear_core::id::RemoteId;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    // Mimic handle_connected: for each connected client, spawn a player entity
    let client_of_0 = stepper.client_of_entities[0];
    let client_of_1 = stepper.client_of_entities[1];
    let client_0_id = stepper.client_of(0).get::<RemoteId>().unwrap().0;
    let client_1_id = stepper.client_of(1).get::<RemoteId>().unwrap().0;

    // Spawn entity for client 0 (like handle_connected would)
    let entity_for_0 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_0_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_0_id)),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
        ))
        .id();
    info!(
        "Spawned entity_for_0: {:?} targeting client {:?}",
        entity_for_0, client_0_id
    );

    // Let it replicate
    stepper.frame_step(3);

    // Now spawn entity for client 1 (like a second client connecting later)
    let entity_for_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_1_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_1_id)),
            ControlledBy {
                owner: client_of_1,
                lifetime: Default::default(),
            },
        ))
        .id();
    info!(
        "Spawned entity_for_1: {:?} targeting client {:?}",
        entity_for_1, client_1_id
    );

    stepper.frame_step(3);

    // Check client 0
    let mm_0 = stepper.client(0).get::<MessageManager>().unwrap();
    let e0_on_c0 = mm_0
        .entity_mapper
        .get_local(entity_for_0)
        .expect("entity_for_0 not replicated to client 0");
    let e1_on_c0 = mm_0
        .entity_mapper
        .get_local(entity_for_1)
        .expect("entity_for_1 not replicated to client 0");

    let e0_predicted = stepper.client_apps[0]
        .world()
        .get::<Predicted>(e0_on_c0)
        .is_some();
    let e0_interpolated = stepper.client_apps[0]
        .world()
        .get::<Interpolated>(e0_on_c0)
        .is_some();
    let e0_controlled = stepper.client_apps[0]
        .world()
        .get::<Controlled>(e0_on_c0)
        .is_some();
    let e1_predicted = stepper.client_apps[0]
        .world()
        .get::<Predicted>(e1_on_c0)
        .is_some();
    let e1_interpolated = stepper.client_apps[0]
        .world()
        .get::<Interpolated>(e1_on_c0)
        .is_some();
    let e1_controlled = stepper.client_apps[0]
        .world()
        .get::<Controlled>(e1_on_c0)
        .is_some();

    info!(
        "Client 0: entity_for_0 ({:?}): predicted={}, interpolated={}, controlled={}",
        e0_on_c0, e0_predicted, e0_interpolated, e0_controlled
    );
    info!(
        "Client 0: entity_for_1 ({:?}): predicted={}, interpolated={}, controlled={}",
        e1_on_c0, e1_predicted, e1_interpolated, e1_controlled
    );

    assert!(e0_predicted, "Client 0's own entity should be Predicted");
    assert!(
        !e0_interpolated,
        "Client 0's own entity should NOT be Interpolated"
    );
    assert!(
        !e1_predicted,
        "Client 1's entity on client 0 should NOT be Predicted"
    );
    assert!(
        e1_interpolated,
        "Client 1's entity on client 0 should be Interpolated"
    );

    // Check client 1
    let mm_1 = stepper.client(1).get::<MessageManager>().unwrap();
    let e0_on_c1 = mm_1
        .entity_mapper
        .get_local(entity_for_0)
        .expect("entity_for_0 not replicated to client 1");
    let e1_on_c1 = mm_1
        .entity_mapper
        .get_local(entity_for_1)
        .expect("entity_for_1 not replicated to client 1");

    let e0_predicted_c1 = stepper.client_apps[1]
        .world()
        .get::<Predicted>(e0_on_c1)
        .is_some();
    let e0_interpolated_c1 = stepper.client_apps[1]
        .world()
        .get::<Interpolated>(e0_on_c1)
        .is_some();
    let e1_predicted_c1 = stepper.client_apps[1]
        .world()
        .get::<Predicted>(e1_on_c1)
        .is_some();
    let e1_interpolated_c1 = stepper.client_apps[1]
        .world()
        .get::<Interpolated>(e1_on_c1)
        .is_some();

    info!(
        "Client 1: entity_for_0 ({:?}): predicted={}, interpolated={}",
        e0_on_c1, e0_predicted_c1, e0_interpolated_c1
    );
    info!(
        "Client 1: entity_for_1 ({:?}): predicted={}, interpolated={}",
        e1_on_c1, e1_predicted_c1, e1_interpolated_c1
    );

    assert!(
        !e0_predicted_c1,
        "Client 0's entity on client 1 should NOT be Predicted"
    );
    assert!(
        e0_interpolated_c1,
        "Client 0's entity on client 1 should be Interpolated"
    );
    assert!(e1_predicted_c1, "Client 1's own entity should be Predicted");
    assert!(
        !e1_interpolated_c1,
        "Client 1's own entity should NOT be Interpolated"
    );

    // Count
    let predicted_count_0 = stepper.client_apps[0]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[0].world())
        .count();
    let predicted_count_1 = stepper.client_apps[1]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[1].world())
        .count();
    assert_eq!(
        predicted_count_0, 1,
        "Client 0 should have exactly 1 Predicted entity, got {predicted_count_0}"
    );
    assert_eq!(
        predicted_count_1, 1,
        "Client 1 should have exactly 1 Predicted entity, got {predicted_count_1}"
    );
}

/// Test prediction visibility with a late-joining client.
/// Client 0 connects first, entity spawned for it. Then client 1 joins.
/// Verifies that client 1 does NOT see client 0's entity as Predicted.
#[test]
fn test_prediction_target_visibility_late_join() {
    use lightyear_core::id::RemoteId;

    // Start with 1 client
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_0_id = stepper.client_of(0).get::<RemoteId>().unwrap().0;
    let client_of_0 = stepper.client_of_entities[0];

    // Spawn entity for client 0
    let entity_for_0 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_0_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_0_id)),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
        ))
        .id();

    stepper.frame_step(3);

    // Verify client 0 sees the entity as Predicted
    let e0_on_c0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_0)
        .unwrap();
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e0_on_c0)
            .is_some(),
        "Client 0 should see its own entity as Predicted"
    );

    // Now client 1 connects (late join)
    stepper.new_client(ClientType::Netcode, None);
    stepper.init();

    let client_1_id = stepper.client_of(1).get::<RemoteId>().unwrap().0;
    let client_of_1 = stepper.client_of_entities[1];

    // Spawn entity for client 1
    let entity_for_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_1_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_1_id)),
            ControlledBy {
                owner: client_of_1,
                lifetime: Default::default(),
            },
        ))
        .id();

    stepper.frame_step(3);

    // Check entity_for_0 on client 1 (late joiner)
    let e0_on_c1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_0)
        .expect("entity_for_0 should be replicated to client 1");
    let e1_on_c1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_1)
        .expect("entity_for_1 should be replicated to client 1");

    let e0_pred_c1 = stepper.client_apps[1]
        .world()
        .get::<Predicted>(e0_on_c1)
        .is_some();
    let e0_interp_c1 = stepper.client_apps[1]
        .world()
        .get::<Interpolated>(e0_on_c1)
        .is_some();
    let e1_pred_c1 = stepper.client_apps[1]
        .world()
        .get::<Predicted>(e1_on_c1)
        .is_some();
    let e1_interp_c1 = stepper.client_apps[1]
        .world()
        .get::<Interpolated>(e1_on_c1)
        .is_some();

    info!(
        "Late-join Client 1: entity_for_0: predicted={}, interpolated={}",
        e0_pred_c1, e0_interp_c1
    );
    info!(
        "Late-join Client 1: entity_for_1: predicted={}, interpolated={}",
        e1_pred_c1, e1_interp_c1
    );

    assert!(
        !e0_pred_c1,
        "Client 0's entity should NOT be Predicted on late-joining client 1"
    );
    assert!(
        e0_interp_c1,
        "Client 0's entity should be Interpolated on late-joining client 1"
    );
    assert!(e1_pred_c1, "Client 1's own entity should be Predicted");
    assert!(
        !e1_interp_c1,
        "Client 1's own entity should NOT be Interpolated"
    );

    // Also check entity_for_1 on client 0
    let e1_on_c0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_1)
        .expect("entity_for_1 should be replicated to client 0");

    let e1_pred_c0 = stepper.client_apps[0]
        .world()
        .get::<Predicted>(e1_on_c0)
        .is_some();
    let e1_interp_c0 = stepper.client_apps[0]
        .world()
        .get::<Interpolated>(e1_on_c0)
        .is_some();

    info!(
        "Client 0: entity_for_1: predicted={}, interpolated={}",
        e1_pred_c0, e1_interp_c0
    );

    assert!(
        !e1_pred_c0,
        "Client 1's entity should NOT be Predicted on client 0"
    );
    assert!(
        e1_interp_c0,
        "Client 1's entity should be Interpolated on client 0"
    );

    // Count predicted per client
    let pred_count_0 = stepper.client_apps[0]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[0].world())
        .count();
    let pred_count_1 = stepper.client_apps[1]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[1].world())
        .count();
    assert_eq!(
        pred_count_0, 1,
        "Client 0 should have exactly 1 Predicted, got {pred_count_0}"
    );
    assert_eq!(
        pred_count_1, 1,
        "Client 1 should have exactly 1 Predicted, got {pred_count_1}"
    );
}

/// Mimics the simple_box example most closely: entities spawned via observer
/// on Connected, with ControlledBy + PredictionTarget/InterpolationTarget.
/// Client 1 connects late (after client 0's entity is already replicated).
#[test]
fn test_prediction_visibility_observer_spawn_late_join() {
    use lightyear_connection::client::Connected;
    use lightyear_connection::client_of::ClientOf;
    use lightyear_core::id::RemoteId;

    // Start with 1 client only
    let mut config = StepperConfig::single();
    config.init = false;
    let mut stepper = ClientServerStepper::from_config(config);

    // Register observer on the SERVER that spawns player entities when clients connect
    // (mimicking simple_box handle_connected)
    stepper.server_app.add_observer(
        |trigger: bevy::prelude::On<bevy::prelude::Add, Connected>,
         query: bevy::prelude::Query<
            &RemoteId,
            bevy::prelude::With<lightyear_connection::client_of::ClientOf>,
        >,
         mut commands: bevy::prelude::Commands| {
            let Ok(remote_id) = query.get(trigger.entity) else {
                return;
            };
            let client_id = remote_id.0;
            info!(
                "Observer: spawning player for client {:?} (client_of {:?})",
                client_id, trigger.entity
            );
            commands.spawn((
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
                ControlledBy {
                    owner: trigger.entity,
                    lifetime: Default::default(),
                },
                Name::from(format!("Player_{:?}", client_id)),
            ));
        },
    );

    stepper.init();

    // Client 0 is now connected and should have triggered the observer
    stepper.frame_step(3);

    // Verify client 0 has 1 predicted entity (its own)
    let pred_count_0 = stepper.client_apps[0]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[0].world())
        .count();
    assert_eq!(
        pred_count_0, 1,
        "After first client connects, client 0 should have 1 Predicted entity, got {pred_count_0}"
    );

    // Now client 1 joins late
    stepper.new_client(ClientType::Netcode, None);
    stepper.init();
    stepper.frame_step(3);

    // Each client should have exactly 1 predicted entity
    let pred_count_0 = stepper.client_apps[0]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[0].world())
        .count();
    let pred_count_1 = stepper.client_apps[1]
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
        .iter(stepper.client_apps[1].world())
        .count();

    info!("Client 0 predicted count: {pred_count_0}");
    info!("Client 1 predicted count: {pred_count_1}");

    // Log details for each client
    for (client_idx, client_app) in stepper.client_apps.iter_mut().enumerate() {
        let world = client_app.world_mut();
        let predicted_entities: Vec<bevy::prelude::Entity> = world
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Predicted>>()
            .iter(world)
            .collect();
        let interpolated_entities: Vec<bevy::prelude::Entity> = world
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<Interpolated>>()
            .iter(world)
            .collect();
        info!(
            "Client {}: predicted={:?}, interpolated={:?}",
            client_idx, predicted_entities, interpolated_entities
        );
    }

    assert_eq!(
        pred_count_0, 1,
        "Client 0 should have exactly 1 Predicted entity, got {pred_count_0}"
    );
    assert_eq!(
        pred_count_1, 1,
        "Client 1 should have exactly 1 Predicted entity, got {pred_count_1}"
    );
}

/// Test that re-inserting a Replicate component works as expected (doesn't
/// create duplicate entities)
/// https://github.com/cBournhonesque/lightyear/issues/1025
/// TODO: crossbeam channel disconnects during Replicate re-insertion with replicon
#[test]
#[ignore]
fn test_reinsert_replicate() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_sender = stepper.client(0).id();
    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
        .id();
    // TODO: might need to step more when syncing to avoid receiving updates from the past?
    stepper.frame_step(1);
    stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");

    // assert!(
    //     stepper
    //         .client_app()
    //         .world()
    //         .get::<ReplicationState>(client_entity)
    //         .unwrap()
    //         .state()
    //         .contains_key(&client_sender)
    // );

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(Replicate::to_server());
    stepper.frame_step(1);

    // TODO
    // assert!(
    //     stepper
    //         .client_app()
    //         .world()
    //         .get::<ReplicationState>(client_entity)
    //         .unwrap()
    //         .state()
    //         .contains_key(&client_sender)
    // );
}

/// Test that verifies Predicted and Interpolated are correctly distributed
/// across clients. Each client's own entity should be Predicted, and other
/// clients' entities should be Interpolated.
#[test]
fn test_server_side_visibility_bits() {
    use lightyear_core::id::RemoteId;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_0_id = stepper.client_of(0).get::<RemoteId>().unwrap().0;
    let client_1_id = stepper.client_of(1).get::<RemoteId>().unwrap().0;
    let client_of_0 = stepper.client_of_entities[0];
    let client_of_1 = stepper.client_of_entities[1];

    // Spawn entity for client 0 (like simple_box's handle_connected observer)
    let entity_for_0 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_0_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_0_id)),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
        ))
        .id();

    // Spawn entity for client 1
    let entity_for_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_1_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_1_id)),
            ControlledBy {
                owner: client_of_1,
                lifetime: Default::default(),
            },
        ))
        .id();

    // Step frames to replicate to clients
    stepper.frame_step(4);

    // Verify on client 0
    let e0_on_c0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_0)
        .unwrap();
    let e1_on_c0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_1)
        .unwrap();

    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e0_on_c0)
            .is_some(),
        "Client 0 should see entity_for_0 as Predicted"
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Interpolated>(e0_on_c0)
            .is_none(),
        "Client 0 should NOT see entity_for_0 as Interpolated"
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Predicted>(e1_on_c0)
            .is_none(),
        "Client 0 should NOT see entity_for_1 as Predicted"
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .get::<Interpolated>(e1_on_c0)
            .is_some(),
        "Client 0 should see entity_for_1 as Interpolated"
    );

    // Verify on client 1
    let e0_on_c1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_0)
        .unwrap();
    let e1_on_c1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_for_1)
        .unwrap();

    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(e0_on_c1)
            .is_none(),
        "Client 1 should NOT see entity_for_0 as Predicted"
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(e0_on_c1)
            .is_some(),
        "Client 1 should see entity_for_0 as Interpolated"
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Predicted>(e1_on_c1)
            .is_some(),
        "Client 1 should see entity_for_1 as Predicted"
    );
    assert!(
        stepper.client_apps[1]
            .world()
            .get::<Interpolated>(e1_on_c1)
            .is_none(),
        "Client 1 should NOT see entity_for_1 as Interpolated"
    );
}
