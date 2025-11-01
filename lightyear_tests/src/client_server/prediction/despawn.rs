use crate::protocol::{CompFull, CompSimple};
use crate::stepper::*;
use bevy::prelude::Component;
use lightyear::prelude::*;

#[derive(Component, Debug, PartialEq)]
struct TestComponent(usize);

/// Test that if a predicted entity gets despawned erroneously
/// The rollback re-adds the predicted entity.
#[test]
fn test_despawned_predicted_rollback() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

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
    let predicted_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

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

    assert!(
        stepper
            .client_app()
            .world()
            .get::<Replicated>(predicted_entity)
            .is_some()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Predicted>(predicted_entity)
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
