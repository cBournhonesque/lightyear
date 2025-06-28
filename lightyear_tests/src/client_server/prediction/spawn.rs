use crate::stepper::ClientServerStepper;
use bevy::ecs::hierarchy::ChildOf;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::{Confirmed, PredictionTarget, Replicate};
use test_log::test;
use tracing::info;

/// https://github.com/cBournhonesque/lightyear/issues/627
/// Test that when we spawn a parent + child with hierarchy (ParentSync),
/// the parent-child hierarchy is maintained on the predicted entities
///
/// Flow:
/// 1) Parent/Child get spawned on client
/// 2) All components are inserted on child, including ParentSync (which is mapped correctly)
///    and ShouldBePredicted
/// 3) In PredictionSet::Spawn, child-predicted is spawned, and Confirmed is added on child
/// 4) Because Confirmed is added, we send an event to sync components from Confirmed to child-predicted
///    NOTE: we cannot sync the components at this point, because the parent-predicted entity is not spawned
///    so the ParentSync component cannot be mapped properly when it's synced to the child-predicted entity!
///
/// We want to make sure that the order is
/// "replicate-components -> spawn-prediction (for both child/parent) -> sync components (including ParentSync) -> update hierarchy"
/// instead of
/// "replicate-components -> spawn-prediction (for child) -> sync components (including ParentSync)
///   -> spawn-prediction (for parent) -> sync components -> update hierarchy"
#[test]
fn test_spawn_predicted_with_hierarchy() {
    let mut stepper = ClientServerStepper::single();

    let server_child = stepper.server_app.world_mut().spawn_empty().id();
    let server_parent = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .add_child(server_child)
        .id();
    stepper.frame_step(2);

    // check that the parent and child are spawned on the client
    let confirmed_child = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_child)
        .expect("child entity was not replicated to client");
    let confirmed_parent = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_parent)
        .expect("parent entity was not replicated to client");
    info!("parent: {confirmed_parent:?}, child: {confirmed_child:?}");

    // check that the parent-child hierarchy is maintained
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<ChildOf>(confirmed_child)
            .expect("confirmed child entity doesn't have a parent")
            .parent(),
        confirmed_parent
    );

    let predicted_child = stepper
        .client_app()
        .world()
        .get::<Confirmed>(confirmed_child)
        .unwrap()
        .predicted
        .expect("confirmed child entity doesn't have a predicted entity");
    let predicted_parent = stepper
        .client_app()
        .world()
        .get::<Confirmed>(confirmed_parent)
        .unwrap()
        .predicted
        .expect("confirmed parent entity doesn't have a predicted entity");

    // check that the parent-child hierarchy is present on the predicted entities
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<ChildOf>(predicted_child)
            .expect("predicted child entity doesn't have a parent")
            .parent(),
        predicted_parent
    );
}
