use crate::protocol::{CompDelta, CompFull};
use crate::stepper::*;
use bevy::prelude::default;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_link::Link;
use lightyear_link::prelude::LinkConditionerConfig;
use lightyear_messages::MessageManager;
use lightyear_replication::delta::{DeltaComponentHistory, DeltaManager};
use lightyear_replication::prelude::{
    Replicate, ReplicationGroup, ReplicationGroupId, ReplicationSender,
};
use lightyear_replication::registry::ComponentKind;
use tracing::info;

#[test]
fn test_component_insert() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::All))
        .id();
    stepper.frame_step_server_first(1);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();

    assert!(stepper.client_of(0).get::<DeltaManager>().is_none());
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(1));
    stepper.frame_step_server_first(1);
    let server_tick = stepper.server_tick();
    assert!(
        stepper
            .server()
            .get::<DeltaManager>()
            .unwrap()
            .get(server_entity, server_tick, ComponentKind::of::<CompDelta>())
            .is_some()
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(1)
    );
}

#[test]
fn test_component_update() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::All))
        .id();
    stepper.frame_step_server_first(1);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(1));
    stepper.frame_step_server_first(1);
    let server_tick_insert = stepper.server_tick();
    let client_tick_insert = stepper.client_tick(0);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(1)
    );
    // TODO: we currently don't receive delta acks for ActionsMessage components
    //  so this update will still be replicated as FromBase
    // check that the receiver added a DeltaComponentHistory
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<DeltaComponentHistory<CompDelta>>(client_entity)
            .unwrap()
            .buffer
            .get(&server_tick_insert)
            .unwrap(),
        &CompDelta(1)
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(2));
    stepper.frame_step_server_first(1);
    let server_tick_update_1 = stepper.server_tick();
    let client_tick_update_1 = stepper.client_tick(0);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(2)
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<DeltaComponentHistory<CompDelta>>(client_entity)
            .unwrap()
            .buffer
            .get(&server_tick_update_1)
            .unwrap(),
        &CompDelta(2)
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(3));
    stepper.frame_step_server_first(1);
    let server_tick_update_2 = stepper.server_tick();
    let client_tick_update_2 = stepper.client_tick(0);
    // we should have received an ack for the update 1, so the delta manager
    //  should have removed old ticks
    assert!(
        stepper
            .server()
            .get::<DeltaManager>()
            .unwrap()
            .get(
                server_entity,
                server_tick_insert,
                ComponentKind::of::<CompDelta>()
            )
            .is_none()
    );
    // the ack tick update is still present
    assert!(
        stepper
            .server()
            .get::<DeltaManager>()
            .unwrap()
            .get(
                server_entity,
                server_tick_update_1,
                ComponentKind::of::<CompDelta>()
            )
            .is_some()
    );
    // the server sent us a FromPrevious() delta update, so the delta-history can
    //  also clear ticks that are older than that
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(3)
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DeltaComponentHistory<CompDelta>>(client_entity)
            .unwrap()
            .buffer
            .get(&server_tick_insert)
            .is_none()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DeltaComponentHistory<CompDelta>>(client_entity)
            .unwrap()
            .buffer
            .get(&server_tick_update_1)
            .is_some()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DeltaComponentHistory<CompDelta>>(client_entity)
            .unwrap()
            .buffer
            .get(&server_tick_update_2)
            .is_some()
    );
}

/// We want to test the following case:
/// - server sends a diff between ticks 1-3
/// - client receives that and applies it
/// - server sends a diff between ticks 1-5 (because the server hasn't received the
///   ack for tick 3 yet)
/// - client receives that, applies it, and it still works even if client was already on tick 3
///   because client will fetch the state for tick 1 from its `ComponentDeltaHistory` before applying the diff
///
/// We can emulate this by adding some delay on the server receiving client packets via the link conditioner.
#[test]
fn test_client_use_component_history() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let kind = ComponentKind::of::<CompDelta>();
    let server_recv_delay: i16 = 2;

    stepper
        .client_of_mut(0)
        .get_mut::<Link>()
        .unwrap()
        .recv
        .conditioner = Some(lightyear_link::LinkConditioner::new(
        LinkConditionerConfig {
            incoming_latency: TICK_DURATION * (server_recv_delay as u32),
            ..default()
        },
    ));

    let group_id = ReplicationGroupId(10);
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            CompDelta(1),
            ReplicationGroup::new_id(10),
        ))
        .id();
    stepper.frame_step_server_first(1);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();
    // Force an update because currently delta-compression doesn't do diffs w.r.t to ActionsMessages.
    info!("Setting start state on server");
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(2));
    stepper.frame_step_server_first(1);
    let server_start_tick = stepper.server_tick();

    // make sure that the server received an ack
    stepper.frame_step_server_first(10);
    // the ack tick is delayed because the server kept sending other updates while it was waiting
    // for the ack from the client
    let ack_tick = server_start_tick + server_recv_delay;
    assert_eq!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .group_channels
            .get(&group_id)
            .unwrap()
            .delta_ack_ticks
            .get(&(server_entity, kind))
            .unwrap(),
        &ack_tick
    );

    // we send an update from the `server_base_tick`: the client will receive it and apply the diff immediately,
    // but the server still doesn't know that this message has been received
    info!("Sending first diff from start state");
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(3));
    stepper.frame_step_server_first(1);
    // confirm that the ack_tick didn't change
    assert_eq!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .group_channels
            .get(&group_id)
            .unwrap()
            .delta_ack_ticks
            .get(&(server_entity, kind))
            .unwrap(),
        &ack_tick
    );
    // confirm that the client applied the diff already
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(3)
    );

    // We send another update from the `server_base_tick` (since we haven't received the ack from CompDelta(3) yet)
    // The client should still apply the diff correctly from its ComponentDeltaHistory
    info!("Sending second diff from start state");
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(CompDelta(4));
    stepper.frame_step_server_first(1);
    assert_eq!(
        stepper
            .client_of(0)
            .get::<ReplicationSender>()
            .unwrap()
            .group_channels
            .get(&group_id)
            .unwrap()
            .delta_ack_ticks
            .get(&(server_entity, kind))
            .unwrap(),
        &ack_tick
    );
    // confirm that the client applied the diff correctly
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(4)
    );
}

/// Test that checks that correctness depends on having a delta_tick
/// per (entity, component):
/// - entities 1 and 2 in the same ReplicationGroup
/// - tick 1: entity 1 sends C-Delta1, which gets lost
/// - tick 2: entity 2 sends C2
/// If we had one ack_tick per group, then the ack tick for the group would be tick 2,
/// and we would fail because we cannot compute a diff from tick 2
#[test]
fn test_update_requires_per_component_entity_ack_ticks() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity_1 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            CompDelta(1),
            ReplicationGroup::new_id(1),
        ))
        .id();
    let server_entity_2 = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            CompFull(1.0),
            ReplicationGroup::new_id(1),
        ))
        .id();
    stepper.frame_step_server_first(1);
    let client_entity_1 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .unwrap();

    // force an insert to ensure that the server has a delta tick
    // (we only update the delta_tick for UpdateMessages, not ActionMessages)
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity_1)
        .insert(CompDelta(1));
    stepper.frame_step_server_first(1);

    // do an update on entity_2; if the ack_ticks were shared for the group,
    // then the next delta-update would be from this tick.
    // It would fail, since we haven't stored a state for this tick.
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity_2)
        .insert(CompFull(2.0));
    stepper.frame_step_server_first(1);

    // however we are able to correctly send a diff
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity_1)
        .insert(CompDelta(2));
    stepper.frame_step_server_first(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity_1)
            .get::<CompDelta>()
            .expect("component missing"),
        &CompDelta(2)
    );
}
