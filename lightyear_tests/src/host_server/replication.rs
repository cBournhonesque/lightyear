use crate::stepper::*;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prediction::Predicted;
use lightyear_replication::authority::HasAuthority;
use lightyear_replication::control::{Controlled, ControlledBy};
use lightyear_replication::prelude::*;
use lightyear_replication::send::ReplicatedFrom;
use test_log::test;

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
