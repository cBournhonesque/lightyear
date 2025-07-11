use crate::protocol::{CompFull, CompSimple};
use crate::stepper::ClientServerStepper;
use bevy::prelude::Component;
use lightyear_connection::prelude::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_prediction::despawn::{PredictionDespawnCommandsExt, PredictionDisable};
use lightyear_prediction::prelude::PredictionManager;
use lightyear_replication::prelude::{Confirmed, PredictionTarget, Replicate};

#[derive(Component, Debug, PartialEq)]
struct TestComponent(usize);

/// Test that if a predicted entity gets despawned erroneously
/// The rollback re-adds the predicted entity.
#[test]
fn test_despawned_predicted_rollback() {
    let mut stepper = ClientServerStepper::single();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            CompFull(1.0),
            CompSimple(1.0),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);
    let confirmed_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    let confirmed = stepper
        .client_app()
        .world()
        .entity(confirmed_entity)
        .get::<Confirmed>()
        .expect("Confirmed component missing");
    let predicted_entity = confirmed.predicted.unwrap();
    // check that a rollback occurred to add the components on the predicted entity
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_entity)
            .unwrap(),
        &CompFull(1.0)
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompSimple>(predicted_entity)
            .unwrap(),
        &CompSimple(1.0)
    );
    // try adding a non-protocol component (which could be some rendering component)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_entity)
        .insert(TestComponent(1));

    // despawn the predicted entity locally
    stepper
        .client_app()
        .world_mut()
        .commands()
        .entity(predicted_entity)
        .prediction_despawn();
    stepper.frame_step(1);
    // make sure that the entity is disabled
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(predicted_entity)
            .is_ok()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionDisable>(predicted_entity)
            .is_some()
    );

    // update the server entity to trigger a rollback where the predicted entity should be 're-spawned'
    stepper
        .server_app
        .world_mut()
        .get_mut::<CompFull>(server_entity)
        .unwrap()
        .0 = 2.0;
    stepper.frame_step(2);

    // Check that the entity was rolled back and the PredictionDisable marker was removed
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionDisable>(predicted_entity)
            .is_none()
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_entity)
            .unwrap(),
        &CompFull(2.0)
    );
    // non-Full components are also present
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompSimple>(predicted_entity)
            .unwrap(),
        &CompSimple(1.0)
    );
}

/// Check that when the confirmed entity gets despawned, the predicted entity gets despawned as well
#[test]
fn test_despawned_confirmed() {
    let mut stepper = ClientServerStepper::single();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            CompFull(1.0),
            CompSimple(1.0),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);
    let confirmed_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    // check that prediction
    let confirmed = stepper
        .client_app()
        .world()
        .entity(confirmed_entity)
        .get::<Confirmed>()
        .expect("Confirmed component missing");
    let predicted_entity = confirmed.predicted.unwrap();

    // despawn the confirmed entity
    stepper.client_app().world_mut().despawn(confirmed_entity);
    stepper.frame_step(1);

    // check that the predicted entity got despawned
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(predicted_entity)
            .is_err()
    );
    // check that the confirmed to predicted map got updated
    unsafe {
        assert!(
            stepper
                .client(0)
                .get::<PredictionManager>()
                .unwrap()
                .predicted_entity_map
                .get()
                .as_ref()
                .unwrap()
                .confirmed_to_predicted
                .get(&confirmed_entity)
                .is_none()
        );
    }
}
