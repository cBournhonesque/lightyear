//! Check various replication scenarios between 2 peers only

use crate::protocol::{CompA, CompDisabled, CompReplicateOnce};
use crate::stepper::ClientServerStepper;
use bevy::prelude::{default, Name, ResMut, Resource, Single};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_messages::MessageManager;
use lightyear_replication::control::{ControlledBy, ControlledByRemote};
use lightyear_replication::message::ActionsChannel;
use lightyear_replication::prelude::{
    ComponentReplicationOverride, ComponentReplicationOverrides, Replicate, ReplicationGroupId,
    ReplicationSender,
};
use lightyear_sync::prelude::InputTimeline;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::prelude::Transport;
use test_log::test;

#[test]
fn test_spawn() {
    let mut stepper = ClientServerStepper::single();

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
}

#[test]
fn test_spawn_from_replicate_change() {
    let mut stepper = ClientServerStepper::single();

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
    let mut stepper = ClientServerStepper::single();

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
    stepper.new_client();
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
    let mut stepper = ClientServerStepper::single();

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
    let mut stepper = ClientServerStepper::single();

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
    let mut stepper = ClientServerStepper::single();

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
    let mut stepper = ClientServerStepper::single();

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
    let mut stepper = ClientServerStepper::single();
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
        .client_mut(0)
        .get_mut::<LocalTimeline>()
        .unwrap()
        .apply_duration(tick_duration * ((u16::MAX / 3 + 10) as u32), tick_duration);
    stepper
        .client_of_mut(0)
        .get_mut::<LocalTimeline>()
        .unwrap()
        .apply_duration(tick_duration * ((u16::MAX / 3 + 10) as u32), tick_duration);
    stepper.frame_step(1);

    stepper
        .client_mut(0)
        .get_mut::<LocalTimeline>()
        .unwrap()
        .apply_duration(tick_duration * ((u16::MAX / 3 + 10) as u32), tick_duration);
    stepper
        .client_of_mut(0)
        .get_mut::<LocalTimeline>()
        .unwrap()
        .apply_duration(tick_duration * ((u16::MAX / 3 + 10) as u32), tick_duration);
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
    let mut stepper = ClientServerStepper::single();

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


/// Check that if we remove a non-replicated component, the replicate component does not get removed
#[test]
fn test_component_remove_non_replicated() {
    let mut stepper = ClientServerStepper::single();

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

/// Test that a component removal is not replicated if the component is marked as disabled
#[test]
fn test_component_remove_disabled() {
    let mut stepper = ClientServerStepper::single();

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

    let mut overrides = ComponentReplicationOverrides::<CompA>::default();
    overrides.global_override(ComponentReplicationOverride {
        disable: true,
        ..default()
    });
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(overrides);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .remove::<CompA>();
    stepper.frame_step(1);
    // the removal was not replicated since the component replication was disabled
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
fn test_component_disabled() {
    let mut stepper = ClientServerStepper::single();

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompDisabled(1.0)))
        .id();
    stepper.frame_step(1);

    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompDisabled>()
            .is_none()
    );
}

#[test]
fn test_component_replicate_once() {
    let mut stepper = ClientServerStepper::single();

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

/// Default = replicate_once
/// GlobalOverride = replicate_always
/// PerSenderOverride = replicate_once
#[test]
fn test_component_replicate_once_overrides() {
    let mut stepper = ClientServerStepper::single();

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

    let mut overrides = ComponentReplicationOverrides::<CompReplicateOnce>::default();
    overrides.global_override(ComponentReplicationOverride {
        replicate_always: true,
        ..default()
    });
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(overrides);
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
        &CompReplicateOnce(2.0)
    );

    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<ComponentReplicationOverrides<CompReplicateOnce>>()
        .unwrap()
        .override_for_sender(
            ComponentReplicationOverride {
                replicate_once: true,
                ..default()
            },
            stepper.client_entities[0],
        );
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<CompReplicateOnce>()
        .unwrap()
        .0 = 3.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompReplicateOnce>()
            .expect("component missing"),
        &CompReplicateOnce(2.0)
    );
}

/// Default = disabled
/// GlobalOverride = enabled
/// PerSenderOverride = disabled
#[test]
fn test_component_disabled_overrides() {
    let mut stepper = ClientServerStepper::single();

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompDisabled(1.0)))
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .unwrap();
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompDisabled>()
            .is_none()
    );

    let mut overrides = ComponentReplicationOverrides::<CompDisabled>::default();
    overrides.global_override(ComponentReplicationOverride {
        enable: true,
        ..default()
    });
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(overrides);
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<CompDisabled>()
        .unwrap()
        .0 = 2.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompDisabled>()
            .expect("component missing"),
        &CompDisabled(2.0)
    );

    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<ComponentReplicationOverrides<CompDisabled>>()
        .unwrap()
        .override_for_sender(
            ComponentReplicationOverride {
                disable: true,
                ..default()
            },
            stepper.client_entities[0],
        );
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<CompDisabled>()
        .unwrap()
        .0 = 3.0;
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<CompDisabled>()
            .expect("component missing"),
        &CompDisabled(2.0)
    );
}

#[test]
fn test_owned_by() {
    let mut stepper = ClientServerStepper::with_clients(2);

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

#[derive(Resource)]
struct ActionsCount(usize);

fn intercept_message(transport: Single<&mut Transport>, mut actions_count: ResMut<ActionsCount>) {
    actions_count.0 += transport
        .senders
        .get(&ChannelKind::of::<ActionsChannel>())
        .unwrap()
        .messages_sent
        .len();
}

/// Test that ReplicationMode::SinceLastAck is respected
/// - we keep sending replication packets until we receive an Ack
#[test]
#[ignore]
fn test_since_last_ack() {
    let mut stepper = ClientServerStepper::single();

    // TODO: how to confirm that a message has been sent?
    let actions_sent = |stepper: &ClientServerStepper| {
        stepper
            .client(0)
            .get::<Transport>()
            .unwrap()
            .senders
            .get(&ChannelKind::of::<ActionsChannel>())
            .unwrap()
            .messages_sent
            .len()
    };

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((Replicate::to_server(), CompA(1.0)))
        .id();

    let tick_duration = stepper.tick_duration;
    stepper.advance_time(tick_duration);

    // send once to the server
    stepper.client_app().update();

    // check that we sent an EntityActions message
    assert_eq!(actions_sent(&stepper), 1);

    stepper.advance_time(tick_duration);

    // check that we send again to the server since we haven't received an ack
    stepper.client_app().update();
    // check that we re-sent an EntityActions message since we didn't receive any acks
    assert_eq!(actions_sent(&stepper), 1);

    // server receives the message and sends back an ack
    stepper.server_app.update();

    stepper.frame_step(1);

    // check that this time we don't send an EntityActions message since our last message has been acked.
    assert_eq!(actions_sent(&stepper), 0);
    let group_id = ReplicationGroupId(client_entity.to_bits());
    let group_channel = stepper
        .client(0)
        .get::<ReplicationSender>()
        .unwrap()
        .group_channels
        .get(&group_id)
        .unwrap();
    assert_ne!(group_channel.ack_bevy_tick, None);
    //
}
