//! Check various replication scenarios between 2 peers only

use crate::protocol::*;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use crate::stepper::*;
use bevy::prelude::*;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::*;
use test_log::test;
#[allow(unused_imports)]
use tracing::info;
use lightyear_replication::visibility::immediate::VisibilityExt;

#[test]
fn test_spawn_gain_visibility() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn(Replicate::to_server())
        .id();
    // entity is auto-visible after Replicate::to_server()
    stepper.frame_step(1);
    stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity should be auto-visible after spawn");

    // lose visibility
    stepper.client_apps[0]
        .world_mut()
        .commands()
        .lose_visibility(client_entity, stepper.client_entities[0]);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_entity)
            .is_none(),
        "entity should not be visible after lose_visibility"
    );

    // gain visibility again
    stepper.client_apps[0]
        .world_mut()
        .commands()
        .gain_visibility(client_entity, stepper.client_entities[0]);
    stepper.frame_step(1);
    stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity should be visible again after gain_visibility");
}

#[test]
fn test_despawn_lose_visibility() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn(Replicate::to_server())
        .id();
    let sender = stepper.client_entities[0];
    stepper
        .client_app()
        .world_mut()
        .gain_visibility(client_entity, sender);
    // entity is visible because of NetworkVisibility
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();

    // lose visibility: a Despawn message should be sent
    stepper.client_apps[0]
        .world_mut()
        .lose_visibility(client_entity, sender);
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
    stepper.frame_step(2);

    // gain visibility: a spawn message should be sent again
    stepper.client_apps[0]
        .world_mut()
        .commands()
        .gain_visibility(client_entity, sender);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_entity)
            .is_some()
    );
}

/// https://github.com/cBournhonesque/lightyear/issues/637
/// Test that if an entity with NetworkVisibility is despawned, the DespawnMessage
/// is only sent to clients that have visibility on it.
#[test]
fn test_despawn_with_visibility() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let server_entity_0 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper
        .server_app
        .world_mut()
        .commands()
        .gain_visibility(server_entity_0, stepper.client_of_entities[0]);

    let server_entity_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper
        .server_app
        .world_mut()
        .commands()
        .gain_visibility(server_entity_1, stepper.client_of_entities[1]);

    stepper.frame_step(2);
    let client_entity_0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_0)
        .unwrap();
    let client_entity_1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .unwrap();

    // update the entity_map on the second connection to re-use the same entity
    // as client 0, so that we can check if the second connection also receives a despawn
    // when we despawn the entity for the first connection.
    //
    // i.e. a DespawnMessage for server_entity_0 send to all clients will also despawn client_entity_1
    // This only works because the replication group is the same for both entities.
    stepper
        .client_mut(1)
        .get_mut::<MessageManager>()
        .unwrap()
        .entity_mapper
        .insert(server_entity_0, client_entity_1);

    stepper.server_app.world_mut().despawn(server_entity_0);
    stepper.frame_step(2);

    // client entity 0 has been despawned
    assert!(
        stepper.client_apps[0]
            .world()
            .get_entity(client_entity_0)
            .is_err()
    );

    // client entity 1 has not been despawned (because connection 1 did not have visibility on server_entity_0)
    assert!(
        stepper.client_apps[1]
            .world()
            .get_entity(client_entity_1)
            .is_ok()
    );
}

/// Test that when we add NetworkVisibility, the entity is despawned on senders
/// that are not present in the NetworkVisibility?
#[test]
fn test_despawn_add_network_visibility() {}

/// Test that there is no logspam when we despawn an entity with NetworkVisibility
/// but that is not visible to a client
#[test]
fn test_despawn_non_visible_logspam() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::single());
    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            CompFull(1.0),
        ))
        .id();
    let server_child = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            ChildOf(server_parent),
        ))
        .id();

    stepper.frame_step(1);
    info!("Server despawning parent that is not visible to the client");
    stepper.server_app.world_mut().despawn(server_parent);
    stepper.frame_step(10);
}

/// https://github.com/cBournhonesque/lightyear/issues/1347
/// If `lose_visibility` clears metadata in ReplicationState, then multiple calls to `lose_visibility`
/// will remove the authority information and prevent the entity from getting replicated!
#[test]
fn test_spawn_multiple_lose_visibility() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn(Replicate::to_server())
        .id();
    // entity is auto-visible after Replicate::to_server()
    stepper.frame_step(1);
    stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity should be auto-visible after spawn");

    // lose visibility
    stepper.client_apps[0]
        .world_mut()
        .commands()
        .lose_visibility(client_entity, stepper.client_entities[0]);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_entity)
            .is_none(),
        "entity should not be visible after lose_visibility"
    );

    // lose visibility again (idempotent): should not cause issues
    stepper.client_apps[0]
        .world_mut()
        .commands()
        .lose_visibility(client_entity, stepper.client_entities[0]);
    stepper.frame_step(1);

    // gain visibility: we should be able to replicate
    stepper.client_apps[0]
        .world_mut()
        .commands()
        .gain_visibility(client_entity, stepper.client_entities[0]);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_entity)
            .is_some(),
        "entity should be visible again after gain_visibility"
    );
}

/// Test that visibility overrides are NOT reset when ReplicationTarget is replaced.
///
/// Scenario: server spawns entity visible to clients 1 and 2, then lose_visibility for client 2,
/// then re-insert Replicate targeting all clients. Client 2 should still not see the entity.
#[test]
fn test_visibility_persists_on_replication_target_change() {
    let mut stepper: ClientServerStepper =
        ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::All))
        .id();
    stepper.frame_step(2);

    // both clients should see the entity
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
            .is_some(),
        "client 0 should see the entity"
    );
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
            .is_some(),
        "client 1 should see the entity"
    );

    // lose visibility for client 1 (second client, index 1)
    let sender_1 = stepper.client_of_entities[1];
    stepper
        .server_app
        .world_mut()
        .lose_visibility(server_entity, sender_1);
    stepper.frame_step(2);

    // client 1 should no longer see the entity
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
            .is_none(),
        "client 1 should not see the entity after lose_visibility"
    );

    // re-insert Replicate targeting all clients (triggers on_replace + on_insert)
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(Replicate::to_clients(NetworkTarget::All));
    stepper.frame_step(2);

    // client 0 should still see the entity
    assert!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
            .is_some(),
        "client 0 should still see the entity after ReplicationTarget change"
    );
    // client 1 should still NOT see the entity (visibility override persists)
    assert!(
        stepper
            .client(1)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
            .is_none(),
        "client 1 should still not see the entity after ReplicationTarget change"
    );
}
