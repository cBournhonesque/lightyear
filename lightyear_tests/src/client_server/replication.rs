//! Check various replication scenarios between 2 peers only

use crate::protocol::{CompA, CompDisabled, CompReplicateOnce};
use crate::stepper::*;
use bevy::prelude::{Name, default};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::LocalTimeline;
use lightyear_messages::MessageManager;
use lightyear_replication::control::{ControlledBy, ControlledByRemote};
use lightyear_replication::prelude::*;
use lightyear_sync::prelude::InputTimeline;
use test_log::test;
use tracing::info;

#[test]
fn test_spawn() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
        .id();
    // let state = stepper
    //     .client_app()
    //     .world()
    //     .get::<ReplicationState>(client_entity);
    // TODO: might need to step more when syncing to avoid receiving updates from the past?
    stepper.frame_step(1);
    stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");
}

#[test]
fn test_spawn_from_replicate_change() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::manual(vec![]),))
        .id();
    stepper.frame_step(1);
    assert!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_entity)
            .is_none()
    );

    // update replicate to include a new sender
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(Replicate::to_server());
    stepper.frame_step(1);

    stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");
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
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");

    // despawn
    stepper.client_app().world_mut().despawn(client_entity);
    stepper.frame_step(1);

    // check that the entity was despawned
    assert!(
        stepper
            .server_app
            .world()
            .get_entity(server_entity)
            .is_err()
    );
}

#[test]
fn test_despawn_from_replicate_change() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");

    // update replicate to exclude the previous sender
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(Replicate::manual(vec![]));
    stepper.frame_step(1);

    // check that the entity was despawned on the previous sender
    assert!(
        stepper
            .server_app
            .world()
            .get_entity(server_entity)
            .is_err()
    );
}

#[test]
fn test_component_insert() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(),))
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
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(CompA(1.0));
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(1.0)
    );
}

#[test]
fn test_component_update() {
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
        .get_mut::<CompA>()
        .unwrap()
        .0 = 2.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(2.0)
    );
}

/// Test that replicating updates works even if the update happens after tick wrapping
#[test]
fn test_component_update_after_tick_wrap() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    // remove InputTimeline otherwise it will try to resync
    stepper.client_mut(0).remove::<InputTimeline>();

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

    let tick_duration = stepper.tick_duration;
    // we increase the ticks in 2 steps (otherwise we would directly go over tick wrapping and the tick cleanup
    // systems would not run)
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta((u16::MAX / 3 + 10) as i16);
    stepper
        .server_app
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta((u16::MAX / 3 + 10) as i16);
    stepper.frame_step(1);

    stepper
        .client_app()
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta((u16::MAX / 3 + 10) as i16);
    stepper
        .server_app
        .world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta((u16::MAX / 3 + 10) as i16);
    stepper.frame_step(1);

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<CompA>()
        .unwrap()
        .0 = 2.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .expect("component missing"),
        &CompA(2.0)
    );
}

#[test]
fn test_component_remove() {
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
        .remove::<CompA>();
    stepper.frame_step(1);
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompA>()
            .is_none()
    );
}

/// Check that component removes are not replicated if the entity does not have Replicating
#[test]
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
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompA(1.0), Name::from("a")))
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
        .remove::<Name>();
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

#[test]
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

#[test]
fn test_owned_by() {
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
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    // client 1 disconnects
    stepper.disconnect_client();
    stepper.frame_step(2);

    // the entity should get despawned on client 0, because it was owned by the client 1
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_entity)
            .is_err()
    );
}

/// Test that re-inserting a Replicate component works as expected (doesn't
/// create duplicate entities)
/// https://github.com/cBournhonesque/lightyear/issues/1025
#[test]
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
