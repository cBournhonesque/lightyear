use crate::protocol::CompA;
use crate::stepper::*;
use bevy::prelude::{Entity, With};
use bevy_replicon::prelude::Remote;
use lightyear::prelude::{client::Connect, server::Start};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::id::RemoteId;
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prediction::Predicted;
use lightyear_messages::MessageManager;
use lightyear_replication::authority::HasAuthority;
use lightyear_replication::control::{Controlled, ControlledBy};
use lightyear_replication::prelude::*;
use lightyear_replication::send::ReplicatedFrom;
use test_log::test;

fn host_only_stepper_before_host_connect() -> ClientServerStepper {
    let mut config = StepperConfig::from_link_types(vec![ClientType::Host], ServerType::Netcode);
    config.init = false;
    let mut stepper = ClientServerStepper::from_config(config);
    stepper.server_app.finish();
    stepper.server_app.cleanup();
    stepper.server_app.world_mut().trigger(Start {
        entity: stepper.server_entity,
    });
    stepper.server_app.world_mut().flush();
    stepper
}

fn connect_host(stepper: &mut ClientServerStepper) -> Entity {
    let host_client_entity = stepper.host_client_entity.unwrap();
    stepper.server_app.world_mut().trigger(Connect {
        entity: host_client_entity,
    });
    stepper.server_app.world_mut().flush();
    host_client_entity
}

/// In host-server mode, spawning an entity with `Replicate` should add
/// `HasAuthority` (server has authority) and `ReplicatedFrom` (host-client
/// sees the entity as replicated from the host-sender).
#[test]
fn test_replicate_adds_authority_and_replicated_from() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    stepper.frame_step(1);

    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(
        entity_ref.contains::<HasAuthority>(),
        "entity should have HasAuthority in host-server mode"
    );
    assert!(
        entity_ref.contains::<ReplicatedFrom>(),
        "entity should have ReplicatedFrom in host-server mode"
    );
}

/// In host-server mode, spawning an entity with `PredictionTarget` should
/// add the `Predicted` marker component.
#[test]
fn test_prediction_target_adds_predicted() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(1);

    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(
        entity_ref.contains::<Predicted>(),
        "entity should have Predicted in host-server mode"
    );
}

/// In host-server mode, spawning an entity with `InterpolationTarget` should
/// add the `Interpolated` marker component.
#[test]
fn test_interpolation_target_adds_interpolated() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(1);

    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(
        entity_ref.contains::<Interpolated>(),
        "entity should have Interpolated in host-server mode"
    );
}

/// Spawning an entity with `ControlledBy` should automatically add the
/// `Controlled` marker component via `#[require(Controlled)]`.
#[test]
fn test_controlled_by_adds_controlled() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    let host_client_entity = stepper.host_client_entity.unwrap();
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: host_client_entity,
                lifetime: Default::default(),
            },
        ))
        .id();
    stepper.frame_step(1);

    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(
        entity_ref.contains::<Controlled>(),
        "entity should have Controlled via #[require(Controlled)] on ControlledBy"
    );
}

#[test]
fn test_replicate_backfills_when_client_becomes_host_client() {
    let mut stepper = host_only_stepper_before_host_connect();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();
    stepper.frame_step(1);

    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(
        !entity_ref.contains::<HasAuthority>(),
        "without any matching sender yet, existing replicated entities do not get authority eagerly"
    );
    assert!(
        !entity_ref.contains::<ReplicatedFrom>(),
        "ReplicatedFrom should not exist before the client becomes a HostClient"
    );

    connect_host(&mut stepper);
    stepper.frame_step(1);

    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(
        entity_ref.contains::<HasAuthority>(),
        "late host-client backfill should preserve authority"
    );
    assert!(
        entity_ref.contains::<ReplicatedFrom>(),
        "late host-client backfill should add ReplicatedFrom for existing replicated entities"
    );
}

#[test]
fn test_prediction_target_still_has_predicted_when_client_becomes_host_client() {
    let mut stepper = host_only_stepper_before_host_connect();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(1);

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .contains::<Predicted>(),
        "Predicted is currently added independently of host-local backfill"
    );

    connect_host(&mut stepper);
    stepper.frame_step(1);

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .contains::<Predicted>(),
        "Predicted should still be present after the client becomes a HostClient"
    );
}

#[test]
fn test_interpolation_target_still_has_interpolated_when_client_becomes_host_client() {
    let mut stepper = host_only_stepper_before_host_connect();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(1);

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .contains::<Interpolated>(),
        "Interpolated is currently added independently of host-local backfill"
    );

    connect_host(&mut stepper);
    stepper.frame_step(1);

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .contains::<Interpolated>(),
        "Interpolated should still be present after the client becomes a HostClient"
    );
}

#[test]
fn test_controlled_backfills_when_client_becomes_host_client() {
    let mut stepper = host_only_stepper_before_host_connect();
    let host_client_entity = stepper.host_client_entity.unwrap();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: host_client_entity,
                lifetime: Default::default(),
            },
        ))
        .id();
    stepper.frame_step(1);

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .contains::<Controlled>(),
        "Controlled is currently added before the host-local observer runs"
    );

    connect_host(&mut stepper);
    stepper.frame_step(1);

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .contains::<Controlled>(),
        "Controlled should still be present after the client becomes a HostClient"
    );
}

#[test]
fn test_host_owned_entity_does_not_loop_back_and_can_rebroadcast() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());
    let host_id = stepper.host_client().get::<RemoteId>().unwrap().0;

    let host_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_server(), CompA(1.0)))
        .id();

    stepper.frame_step(2);

    let remote_copies = stepper
        .server_app
        .world_mut()
        .query_filtered::<Entity, (With<CompA>, With<Remote>)>()
        .iter(stepper.server_app.world())
        .count();
    assert_eq!(
        remote_copies, 0,
        "host-owned entities should not be looped back through the client-send endpoint"
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(host_entity)
        .insert(Replicate::to_clients(NetworkTarget::AllExceptSingle(
            host_id,
        )));

    stepper.frame_step(2);

    let remote_client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(host_entity)
        .expect("remote client should receive the rebroadcast entity");

    assert_eq!(
        stepper.client_apps[0]
            .world()
            .get::<CompA>(remote_client_entity),
        Some(&CompA(1.0))
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(host_entity)
        .insert(CompA(2.0));
    stepper.frame_step(2);

    assert_eq!(
        stepper.client_apps[0]
            .world()
            .get::<CompA>(remote_client_entity),
        Some(&CompA(2.0)),
        "rebroadcast client should receive subsequent updates from the host-owned entity"
    );
}
