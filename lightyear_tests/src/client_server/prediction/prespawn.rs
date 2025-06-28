use crate::protocol::{CompFull, CompMap, CompSimple};
use crate::stepper::ClientServerStepper;
use bevy::app::PreUpdate;
use bevy::prelude::{Entity, IntoScheduleConfigs, With};
use bevy::utils::default;
use lightyear::prelude::{Link, LinkConditionerConfig, RecvLinkConditioner};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::history_buffer::HistoryState;
use lightyear_messages::MessageManager;
use lightyear_prediction::Predicted;
use lightyear_prediction::despawn::{PredictionDespawnCommandsExt, PredictionDisable};
use lightyear_prediction::diagnostics::PredictionMetrics;
use lightyear_prediction::plugin::{PredictionSet, is_in_rollback};
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::prelude::{PreSpawned, PredictionManager};
use lightyear_replication::prelude::{
    Confirmed, PredictionTarget, Replicate, Replicated, ReplicationGroup,
};
use lightyear_sync::prelude::InputTimeline;
use lightyear_utils::ready_buffer::ItemWithReadyKey;
use test_log::test;
use tracing::info;

#[test]
fn test_compute_hash() {
    let mut stepper = ClientServerStepper::single();

    // check default compute hash, with multiple entities sharing the same tick
    let entity_1 = stepper
        .client_app()
        .world_mut()
        .spawn((CompFull(1.0), PreSpawned::default()))
        .id();
    let entity_2 = stepper
        .client_app()
        .world_mut()
        .spawn((CompFull(1.0), PreSpawned::default()))
        .id();
    stepper.frame_step(1);

    let current_tick = stepper.client_tick(0);
    let prediction_manager = stepper.client(0).get::<PredictionManager>().unwrap();
    let expected_hash: u64 = 276390752926217031;
    assert_eq!(
        prediction_manager
            .prespawn_hash_to_entities
            .get(&expected_hash)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        prediction_manager.prespawn_tick_to_hash.heap.peek(),
        Some(&ItemWithReadyKey {
            // NOTE: in this test we have to add + 1 here because the `register_prespawn_hashes` observer
            //  runs outside of the FixedUpdate schedule so the entity is registered with the previous tick
            //  in a real situation the entity would be spawned inside FixedUpdate so the hash would be correct
            key: current_tick - 1,
            item: expected_hash,
        })
    );

    // check that a PredictionHistory got added to the entity
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(entity_1)
            .get::<PredictionHistory<CompFull>>()
            .unwrap()
            .peek(),
        Some(&(current_tick, HistoryState::Updated(CompFull(1.0)),))
    );
}

/// Prespawning multiple entities with the same hash
/// https://github.com/cBournhonesque/lightyear/issues/906
///
/// This errors only if the server entities were part of the same replication group
#[test]
fn test_multiple_prespawn() {
    let mut stepper = ClientServerStepper::single();

    let client_tick = stepper.client_tick(0).0 as usize;
    let server_tick = stepper.server_tick().0 as usize;
    let client_prespawn_a = stepper
        .client_app()
        .world_mut()
        .spawn(PreSpawned::new(1))
        .id();
    let client_prespawn_b = stepper
        .client_app()
        .world_mut()
        .spawn(PreSpawned::new(1))
        .id();
    // we want to advance by the tick difference, so that the server prespawned is spawned on the same
    // tick as the client prespawned
    // (i.e. entity is spawned on tick client_tick = X on client, and spawned on tick server_tick = X on server, so that
    // the Histories match)
    for tick in server_tick + 1..client_tick {
        stepper.frame_step(1);
    }
    let server_prespawn_a = stepper
        .server_app
        .world_mut()
        .spawn((
            PreSpawned::new(1),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ReplicationGroup::new_id(1),
        ))
        .id();
    let server_prespawn_b = stepper
        .server_app
        .world_mut()
        .spawn((
            PreSpawned::new(1),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ReplicationGroup::new_id(1),
        ))
        .id();
    stepper.frame_step(1);
    stepper.frame_step(1);

    // check that both prespawn entities have been replaced with predicted entities
    let predicted_a = stepper
        .client_app()
        .world()
        .get::<Predicted>(client_prespawn_a)
        .unwrap();
    let confirmed_a = predicted_a.confirmed_entity.unwrap();
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<Confirmed>(confirmed_a)
            .unwrap()
            .predicted
            .unwrap(),
        client_prespawn_a
    );
    // The PreSpawnPlayerObject component has been removed on the client
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PreSpawned>(client_prespawn_a)
            .is_none()
    );

    let predicted_b = stepper
        .client_app()
        .world()
        .get::<Predicted>(client_prespawn_b)
        .unwrap();
    let confirmed_b = predicted_b.confirmed_entity.unwrap();
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<Confirmed>(confirmed_b)
            .unwrap()
            .predicted
            .unwrap(),
        client_prespawn_b
    );
    // The PreSpawnPlayerObject component has been removed on the client
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PreSpawned>(client_prespawn_b)
            .is_none()
    );
}

/// Client and server run the same system to prespawn an entity
/// Server's should take over authority over the entity
#[test]
fn test_prespawn_success() {
    let mut stepper = ClientServerStepper::single();

    let client_prespawn = stepper
        .client_app()
        .world_mut()
        .spawn(PreSpawned::new(1))
        .id();
    let server_prespawn = stepper
        .server_app
        .world_mut()
        .spawn((
            PreSpawned::new(1),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);

    // thanks to pre-spawning, a Confirmed entity has been spawned on the client
    // that Confirmed entity is replicate from server_prespawn
    // and has client_prespawn as predicted entity
    let predicted = stepper
        .client_app()
        .world()
        .get::<Predicted>(client_prespawn)
        .unwrap();
    let confirmed = predicted.confirmed_entity.unwrap();
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<Confirmed>(confirmed)
            .unwrap()
            .predicted
            .unwrap(),
        client_prespawn
    );
    assert_eq!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(server_prespawn)
            .unwrap(),
        confirmed
    );
    // The PreSpawned component has been removed on the client
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PreSpawned>(client_prespawn)
            .is_none()
    );

    // if the Confirmed entity is despawned, the Predicted entity should also be despawned
    stepper.client_app().world_mut().despawn(confirmed);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_prespawn)
            .is_err()
    );
}

/// Client and server run the same system to prespawn an entity
/// The pre-spawn somehow fails on the client (no matching hash)
/// The server entity should just get normally Predicted on the client
///
/// If the Confirmed entity is despawned, the Predicted entity should be despawned
#[test]
fn test_prespawn_client_missing() {
    let mut stepper = ClientServerStepper::single();

    // spawn extra entities to check that EntityMapping works correctly with pre-spawning
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ReplicationGroup::new_id(0),
        ))
        .id();
    stepper.frame_step(2);
    let (client_confirmed, confirmed) = stepper
        .client_app()
        .world_mut()
        .query_filtered::<(Entity, &Confirmed), With<Replicated>>()
        .single(stepper.client_app().world())
        .unwrap();
    let client_predicted = confirmed.predicted.unwrap();

    // run prespawned entity on server.
    // for some reason the entity is not spawned on the client
    let server_entity_2 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ReplicationGroup::new_id(0),
            PreSpawned::default(),
            CompMap(server_entity),
        ))
        .id();
    stepper.frame_step(2);

    // We couldn't match the entity based on hash
    // So we should have just spawned a predicted entity
    let client_confirmed_2 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_2)
        .expect("entity was not replicated to client");
    // it should have a predicted entity
    let client_predicted_2 = stepper
        .client_app()
        .world()
        .get::<Confirmed>(client_confirmed_2)
        .unwrap()
        .predicted
        .unwrap();

    // the MapEntities component should have been mapped to confirmed
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompMap>(client_confirmed_2)
            .unwrap()
            .0,
        client_confirmed
    );
    // the MapEntities component on the predicted entity should have been mapped to predicted
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompMap>(client_predicted_2)
            .unwrap()
            .0,
        client_predicted
    );

    // If we despawn the confirmed entity, the predicted entity should also be despawned
    stepper.client_app().world_mut().despawn(client_confirmed_2);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_predicted_2)
            .is_err()
    );
}

/// Client spawns a PreSpawned entity and tries to despawn it locally
/// before it gets matched to a server entity.
/// The entity should be kept around in case of a match, and then cleanup via the cleanup system.
#[test]
fn test_prespawn_local_despawn_no_match() {
    let mut stepper = ClientServerStepper::single();

    let client_prespawn = stepper
        .client_app()
        .world_mut()
        .spawn((PreSpawned::new(1), CompFull(1.0), CompSimple(1.0)))
        .id();
    stepper.frame_step(1);
    stepper
        .client_app()
        .world_mut()
        .commands()
        .entity(client_prespawn)
        .prediction_despawn();
    stepper.frame_step(1);
    // check that the entity is disabled
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_prespawn)
            .is_ok()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionDisable>(client_prespawn)
            .is_some()
    );

    // if enough frames pass without match, the entity gets cleaned
    stepper.frame_step(60);
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_prespawn)
            .is_err()
    );
}

fn panic_on_rollback() {
    panic!("rollback triggered");
}

/// Client spawns a PreSpawned entity and tries to despawn it locally
/// before it gets matched to a server entity.
/// The match should work normally without causing any rollbacks, since the server components
/// on the PreSpawned entity should match the client history when it was spawned.
#[test]
fn test_prespawn_local_despawn_match() {
    let mut stepper = ClientServerStepper::default_no_init();
    let tick_duration = stepper.tick_duration;
    stepper.new_client();
    // add a conditioner to make sure that the client is ahead of the server, and make sure there is a resync
    stepper
        .client_mut(0)
        .get_mut::<InputTimeline>()
        .unwrap()
        .0
        .context
        .config
        .max_error_margin = 0.5;
    stepper
        .client_mut(0)
        .get_mut::<Link>()
        .unwrap()
        .recv
        .conditioner = Some(RecvLinkConditioner::new(LinkConditionerConfig {
        incoming_latency: 2 * tick_duration,
        ..default()
    }));
    stepper.init();

    stepper.client_app().add_systems(
        PreUpdate,
        panic_on_rollback
            .run_if(is_in_rollback)
            .in_set(PredictionSet::PrepareRollback),
    );

    let client_tick = stepper.client_tick(0).0 as usize;
    let server_tick = stepper.server_tick().0 as usize;
    info!(client_tick, server_tick);
    let client_prespawn = stepper
        .client_app()
        .world_mut()
        .spawn((PreSpawned::new(1), CompFull(1.0), CompSimple(1.0)))
        .id();

    stepper.frame_step(1);

    // do a predicted despawn (we first wait one frame otherwise the components would get removed
    //  immediately and the prediction-history would be empty)
    stepper
        .client_app()
        .world_mut()
        .commands()
        .entity(client_prespawn)
        .prediction_despawn();

    // we want to advance by the tick difference, so that the server prespawned is spawned on the same
    // tick as the client prespawned
    // (i.e. entity is spawned on tick client_tick = X on client, and spawned on tick server_tick = X on server, so that
    // the Histories match)
    stepper.frame_step(client_tick - (server_tick + 1));
    let server_tick = stepper.server_tick().0 as usize;
    info!(server_tick);

    // make sure that the client_prespawn entity was disabled
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(client_prespawn)
            .is_ok()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionDisable>(client_prespawn)
            .is_some()
    );

    // spawn the server prespawned entity
    let server_prespawn = stepper
        .server_app
        .world_mut()
        .spawn((
            PreSpawned::new(1),
            CompFull(1.0),
            CompSimple(1.0),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    // advance enough so that the server entity is received
    stepper.frame_step(5);

    // the server entity gets replicated to the client
    // we should have a match with no rollbacks since the history matches with the confirmed state
    let confirmed = stepper
        .client_app()
        .world()
        .get::<Predicted>(client_prespawn)
        .unwrap();
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<PredictionMetrics>()
            .rollbacks,
        0
    );
}
